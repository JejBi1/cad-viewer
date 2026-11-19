//! DXF IMAGE entities with real picture files next to the drawing: lookup, decoding, z-order, the fallbacks (frame and
//! file name), remote names, clipping, transparency, colour effects and precision far from the origin.
use crate::{load, LoadOptions, OriginMode, SceneOptions};
use cad_types::Scene2d;
use image::{ImageFormat, Rgba, RgbaImage};
use std::path::{Path, PathBuf};

fn tag(code: i32, value: impl std::fmt::Display) -> String {
    format!("{code:>3}\n{value}\n")
}

/// A complete DXF with the given entities and IMAGEDEF objects (`(handle, file name)` pairs).
fn dxf(entities: &str, defs: &[(&str, &str)]) -> String {
    let head = "  0\nSECTION\n  2\nHEADER\n  9\n$ACADVER\n  1\nAC1027\n  9\n$INSUNITS\n 70\n4\n  0\nENDSEC\n";
    let tables = "  0\nSECTION\n  2\nTABLES\n  0\nTABLE\n  2\nLTYPE\n 70\n1\n  0\nLTYPE\n  2\nContinuous\n 70\n0\n  3\nSolid line\n \
                  72\n65\n 73\n0\n 40\n0\n  0\nENDTAB\n  0\nTABLE\n  2\nLAYER\n 70\n1\n  0\nLAYER\n  2\n0\n 70\n0\n 62\n7\n  6\n\
                  Continuous\n  0\nENDTAB\n  0\nTABLE\n  2\nSTYLE\n 70\n1\n  0\nSTYLE\n  2\nStandard\n 70\n0\n 40\n0\n 41\n1\n 50\n0\n  3\n\
                  arial.ttf\n  0\nENDTAB\n  0\nENDSEC\n  0\nSECTION\n  2\nBLOCKS\n  0\nENDSEC\n";
    let mut s = String::from(head);
    s.push_str(tables);
    s.push_str("  0\nSECTION\n  2\nENTITIES\n");
    s.push_str(entities);
    s.push_str("  0\nENDSEC\n  0\nSECTION\n  2\nOBJECTS\n");
    for (handle, name) in defs {
        s.push_str(&tag(0, "IMAGEDEF"));
        s.push_str(&tag(5, handle));
        s.push_str(&tag(1, name));
    }
    s.push_str("  0\nENDSEC\n  0\nEOF\n");
    s
}

/// IMAGE entity: insert point, per-pixel vectors `u` and `v` (drawing units), picture size in pixels, IMAGEDEF handle,
/// flags, and extra group codes.
fn image(def: &str, insert: [f64; 2], u: [f64; 2], v: [f64; 2], size: [f64; 2], flags: i32, extra: &str) -> String {
    let mut s = tag(0, "IMAGE") + &tag(8, "0") + &tag(10, insert[0]) + &tag(20, insert[1]) + &tag(30, 0);
    s += &(tag(11, u[0]) + &tag(21, u[1]) + &tag(31, 0) + &tag(12, v[0]) + &tag(22, v[1]) + &tag(32, 0));
    s += &(tag(13, size[0]) + &tag(23, size[1]) + &tag(340, def) + &tag(70, flags) + &tag(71, 1));
    s + extra
}

/// A closed-looking SOLID with corners (x0, y0) .. (x1, y1).
fn solid(x0: f64, y0: f64, x1: f64, y1: f64) -> String {
    tag(0, "SOLID") + &tag(8, "0") + &tag(10, x0) + &tag(20, y0) + &tag(11, x1) + &tag(21, y0) + &tag(12, x0) + &tag(22, y1) + &tag(13, x1) + &tag(23, y1)
}

fn write_png(path: &Path, w: u32, h: u32, px: impl Fn(u32, u32) -> [u8; 4]) {
    let mut img = RgbaImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            img.put_pixel(x, y, Rgba(px(x, y)));
        }
    }
    img.save_with_format(path, ImageFormat::Png).expect("write png");
}

/// Four colours, red, green, blue and white, top row first.
fn four_colours(x: u32, y: u32) -> [u8; 4] {
    match (x, y) {
        (0, 0) => [255, 0, 0, 255],
        (1, 0) => [0, 255, 0, 255],
        (0, 1) => [0, 0, 255, 255],
        _ => [255, 255, 255, 255],
    }
}

/// Writes the drawing text into a fresh folder and loads it from there (the picture lookup needs a path).
fn load_in(dir: &Path, text: &str) -> crate::Drawing {
    let path: PathBuf = dir.join("plan.dxf");
    std::fs::write(&path, text).expect("write dxf");
    load(&path, &LoadOptions::default(), &mut |_| {}).expect("drawing loads")
}

fn scene(d: &crate::Drawing) -> Scene2d {
    d.scene_with("Model", &SceneOptions::default()).expect("scene")
}

fn fill_count(sc: &Scene2d) -> usize {
    sc.batches.iter().map(|b| b.fills.len()).sum()
}

fn approx(a: [f32; 2], b: [f64; 2]) -> bool {
    (f64::from(a[0]) - b[0]).abs() < 1e-4 && (f64::from(a[1]) - b[1]).abs() < 1e-4
}

#[test]
fn picture_is_decoded_placed_and_kept_in_z_order() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_png(&dir.path().join("pic.png"), 2, 2, four_colours);
    // a SOLID drawn before the picture (covered by it) and the frame lines after it
    let body = solid(0.0, 0.0, 10.0, 10.0) + &image("90", [0.0, 0.0], [5.0, 0.0], [0.0, 5.0], [2.0, 2.0], 3, "");
    let d = load_in(dir.path(), &dxf(&body, &[("90", "pic.png")]));
    let sc = scene(&d);
    assert_eq!(sc.images.len(), 1, "one picture");
    let img = &sc.images[0];
    assert_eq!((img.width, img.height), (2, 2));
    assert_eq!(&img.rgba[..4], &[255, 0, 0, 255], "row 0, column 0 is the top left pixel");
    assert_eq!(&img.rgba[4..8], &[0, 255, 0, 255]);
    assert_eq!(&img.rgba[8..12], &[0, 0, 255, 255], "row 1 is below row 0");
    assert_eq!(&img.rgba[12..16], &[255, 255, 255, 255]);
    // the SOLID is batch 0, the picture comes after it and the frame after the picture
    assert_eq!(img.after, 1);
    assert!(sc.batches[0].fills.len() >= 3 && sc.batches[0].lines.is_empty());
    assert!(sc.batches[1..].iter().any(|b| !b.lines.is_empty()), "the frame is drawn above the picture");
    // corners: top left (0, 10), top right (10, 10), bottom right (10, 0), bottom left (0, 0) with origin 0
    let c = img.corners;
    assert!(approx(c[0], [0.0, 10.0]) && approx(c[1], [10.0, 10.0]) && approx(c[2], [10.0, 0.0]) && approx(c[3], [0.0, 0.0]), "{c:?}");
    assert_eq!(sc.origin, [0.0, 0.0]);
}

#[test]
fn pictures_are_found_in_subfolders_and_by_file_name() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir(dir.path().join("scans")).expect("mkdir");
    write_png(&dir.path().join("scans").join("a.png"), 2, 2, |_, _| [10, 20, 30, 255]);
    write_png(&dir.path().join("b.png"), 2, 2, |_, _| [40, 50, 60, 255]);
    let body = image("90", [0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [2.0, 2.0], 3, "")
        + &image("91", [5.0, 0.0], [1.0, 0.0], [0.0, 1.0], [2.0, 2.0], 3, "")
        + &image("92", [10.0, 0.0], [1.0, 0.0], [0.0, 1.0], [2.0, 2.0], 3, "");
    // relative folder, a missing absolute folder whose file name is found next to the drawing, and an unknown name
    let defs = [("90", "scans\\a.png"), ("91", "Z:\\no-such-folder-here\\b.png"), ("92", "nothere.png")];
    let d = load_in(dir.path(), &dxf(&body, &defs));
    let sc = scene(&d);
    assert_eq!(sc.images.len(), 2, "the unknown name keeps its frame only");
    assert_eq!(&sc.images[0].rgba[..4], &[10, 20, 30, 255]);
    assert_eq!(&sc.images[1].rgba[..4], &[40, 50, 60, 255]);
    assert!(fill_count(&sc) > 0, "the file name of the unknown picture is drawn as text");
}

#[test]
fn missing_or_damaged_pictures_keep_the_frame_and_the_file_name() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("broken.png"), b"\x89PNG but not really").expect("write");
    let shown = image("90", [0.0, 0.0], [5.0, 0.0], [0.0, 5.0], [4.0, 4.0], 3, "");
    let missing = dxf(&shown, &[("90", "missing.png")]);
    let sc = scene(&load_in(dir.path(), &missing));
    assert!(sc.images.is_empty());
    assert!(fill_count(&sc) > 0, "the file name is drawn as text");
    assert!(sc.batches.iter().any(|b| !b.lines.is_empty()), "the frame is drawn");
    let broken = dxf(&shown, &[("90", "broken.png")]);
    let sc = scene(&load_in(dir.path(), &broken));
    assert!(sc.images.is_empty() && fill_count(&sc) > 0);
    // show image flag off: frame only, no name
    let hidden = image("90", [0.0, 0.0], [5.0, 0.0], [0.0, 5.0], [4.0, 4.0], 2, "");
    let sc = scene(&load_in(dir.path(), &dxf(&hidden, &[("90", "missing.png")])));
    assert!(sc.images.is_empty() && fill_count(&sc) == 0);
    // thumbnails (images off): frame only
    let d = load_in(dir.path(), &dxf(&shown, &[("90", "broken.png")]));
    let sc = d.scene_with("Model", &SceneOptions { images: false, ..Default::default() }).expect("scene");
    assert!(sc.images.is_empty() && fill_count(&sc) == 0);
}

#[test]
fn remote_names_block_every_picture_and_no_name_is_drawn() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_png(&dir.path().join("pic.png"), 2, 2, four_colours);
    // a local picture next to the drawing and a UNC name: the whole drawing gets frames only, as in the Python viewer
    let body = image("90", [0.0, 0.0], [5.0, 0.0], [0.0, 5.0], [2.0, 2.0], 3, "")
        + &image("91", [20.0, 0.0], [5.0, 0.0], [0.0, 5.0], [2.0, 2.0], 3, "");
    let d = load_in(dir.path(), &dxf(&body, &[("90", "pic.png"), ("91", "\\\\server\\share\\pic.png")]));
    assert_eq!(d.info().remote_images, vec!["\\\\server\\share\\pic.png".to_string()]);
    let sc = scene(&d);
    assert!(sc.images.is_empty(), "no picture at all");
    assert_eq!(fill_count(&sc), 0, "no file name text either");
    assert!(sc.batches.iter().any(|b| !b.lines.is_empty()), "frames are drawn");
}

/// Portable mode loads a copy of the drawing from the cache; the pictures are next to the original on the Drive.
/// Returns the folder of the original, the path of the copy and the folder that holds the copy.
fn portable_copy(text: &str) -> (tempfile::TempDir, PathBuf, tempfile::TempDir) {
    let original = tempfile::tempdir().expect("tempdir");
    write_png(&original.path().join("pic.png"), 2, 2, four_colours);
    let cache = tempfile::tempdir().expect("tempdir");
    let copy = cache.path().join("plan.dxf");
    std::fs::write(&copy, text).expect("write the copy");
    (original, copy, cache)
}

#[test]
fn a_local_copy_finds_the_pictures_next_to_the_original_drawing() {
    let body = image("90", [0.0, 0.0], [5.0, 0.0], [0.0, 5.0], [2.0, 2.0], 3, "");
    let text = dxf(&body, &[("90", "pic.png")]);
    let (original, copy, _cache) = portable_copy(&text);
    // the copy alone has no picture next to it
    let alone = load(&copy, &LoadOptions::default(), &mut |_| {}).expect("drawing loads");
    assert!(scene(&alone).images.is_empty());
    // with the folder of the original the picture is found and drawn
    let opts = LoadOptions { original_dir: Some(original.path().to_path_buf()), ..Default::default() };
    let d = load(&copy, &opts, &mut |_| {}).expect("drawing loads");
    let sc = scene(&d);
    assert_eq!(sc.images.len(), 1, "the picture next to the original");
    assert_eq!(&sc.images[0].rgba[..4], &[255, 0, 0, 255]);
    // an empty folder is no folder: the folder of the file that is loaded is used
    let opts = LoadOptions { original_dir: Some(PathBuf::new()), ..Default::default() };
    assert!(scene(&load(&copy, &opts, &mut |_| {}).expect("drawing loads")).images.is_empty());
}

#[test]
fn remote_names_are_still_blocked_when_the_pictures_are_looked_up_next_to_the_original() {
    let body = image("90", [0.0, 0.0], [5.0, 0.0], [0.0, 5.0], [2.0, 2.0], 3, "")
        + &image("91", [20.0, 0.0], [5.0, 0.0], [0.0, 5.0], [2.0, 2.0], 3, "");
    let text = dxf(&body, &[("90", "pic.png"), ("91", "\\\\server\\share\\pic.png")]);
    let (original, copy, _cache) = portable_copy(&text);
    let opts = LoadOptions { original_dir: Some(original.path().to_path_buf()), ..Default::default() };
    let d = load(&copy, &opts, &mut |_| {}).expect("drawing loads");
    assert_eq!(d.info().remote_images, vec!["\\\\server\\share\\pic.png".to_string()]);
    let sc = scene(&d);
    assert!(sc.images.is_empty(), "no picture at all, not even the local one");
    assert_eq!(fill_count(&sc), 0, "no file name text either");
    // a remote name given as the only picture is not looked up in the original folder under its file name either
    let body = image("91", [0.0, 0.0], [5.0, 0.0], [0.0, 5.0], [2.0, 2.0], 3, "");
    let (original, copy, _cache) = portable_copy(&dxf(&body, &[("91", "\\\\server\\share\\pic.png")]));
    let opts = LoadOptions { original_dir: Some(original.path().to_path_buf()), ..Default::default() };
    let sc = scene(&load(&copy, &opts, &mut |_| {}).expect("drawing loads"));
    assert!(sc.images.is_empty(), "pic.png next to the original is not the picture of a UNC name");
}

#[test]
fn clipping_mask_and_entity_transparency_change_the_alpha() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_png(&dir.path().join("red.png"), 4, 4, |_, _| [255, 0, 0, 255]);
    // triangle (0,0) (4,0) (0,4) in pixel coordinates (top left origin), clip mode 0: keep the inside; the entity
    // transparency 0x02000080 is alpha 128
    let extra = tag(91, 3) + &tag(14, 0.0) + &tag(24, 0.0) + &tag(14, 4.0) + &tag(24, 0.0) + &tag(14, 0.0) + &tag(24, 4.0) + &tag(290, 0) + &tag(440, 0x0200_0080i64);
    let body = image("90", [0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [4.0, 4.0], 3 | 4 | 8, &extra);
    let d = load_in(dir.path(), &dxf(&body, &[("90", "red.png")]));
    let sc = scene(&d);
    let img = &sc.images[0];
    let alpha = |x: u32, y: u32| img.rgba[((y * 4 + x) * 4 + 3) as usize];
    assert_eq!(alpha(0, 0), 128, "inside the triangle, transparency 128");
    assert_eq!(alpha(1, 1), 128);
    assert_eq!(alpha(3, 3), 0, "outside the triangle: removed");
    assert_eq!(&img.rgba[..3], &[255, 0, 0], "colours are not changed by the mask");
    // the same picture without the clipping flag is opaque everywhere
    let plain = image("90", [0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [4.0, 4.0], 3, "");
    let sc = scene(&load_in(dir.path(), &dxf(&plain, &[("90", "red.png")])));
    assert_eq!(sc.images[0].rgba[3], 255);
    assert_eq!(sc.images[0].rgba[(15 * 4 + 3) as usize], 255);
}

#[test]
fn brightness_follows_the_dxf_code() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_png(&dir.path().join("dark.png"), 2, 2, |_, _| [0, 0, 0, 255]);
    let bright = image("90", [0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [2.0, 2.0], 3, &tag(281, 100));
    let sc = scene(&load_in(dir.path(), &dxf(&bright, &[("90", "dark.png")])));
    assert!(sc.images[0].rgba.chunks_exact(4).all(|p| p[..3] == [255, 255, 255]), "brightness 100 is white");
    let normal = image("90", [0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [2.0, 2.0], 3, "");
    let sc = scene(&load_in(dir.path(), &dxf(&normal, &[("90", "dark.png")])));
    assert!(sc.images[0].rgba.chunks_exact(4).all(|p| p == [0, 0, 0, 255]), "defaults change nothing");
}

#[test]
fn pictures_far_from_the_origin_keep_their_place_in_relative_coordinates() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_png(&dir.path().join("pic.png"), 2, 2, four_colours);
    // S-JTSK like coordinates: a 1 mm picture, 1 000 m from the origin
    let body = image("90", [-742_000.0, -1_043_000.0], [0.5, 0.0], [0.0, 0.5], [2.0, 2.0], 3, "");
    let d = load_in(dir.path(), &dxf(&body, &[("90", "pic.png")]));
    let sc = d.scene_with("Model", &SceneOptions { origin: OriginMode::Center, ..Default::default() }).expect("scene");
    assert!((sc.origin[0] + 741_999.5).abs() <= 1.0 && (sc.origin[1] + 1_042_999.5).abs() <= 1.0, "{:?}", sc.origin);
    let c = sc.images[0].corners;
    // world position = origin + relative corner, exact to far better than a micrometre
    let world = |p: [f32; 2]| [sc.origin[0] + f64::from(p[0]), sc.origin[1] + f64::from(p[1])];
    let bl = world(c[3]);
    assert!((bl[0] + 742_000.0).abs() < 1e-6 && (bl[1] + 1_043_000.0).abs() < 1e-6, "{bl:?}");
    let tr = world(c[1]);
    assert!((tr[0] + 741_999.0).abs() < 1e-6 && (tr[1] + 1_042_999.0).abs() < 1e-6, "{tr:?}");
    assert!(c[1][0].abs() < 1000.0, "relative coordinates are small: {:?}", c[1]);
}

#[test]
fn every_file_is_decoded_once_however_it_is_spelled() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_png(&dir.path().join("pic.png"), 2, 2, four_colours);
    // the same file under 30 spellings, 3 pictures of it in the drawing
    let names: Vec<String> = (0..30).map(|i| format!("{}pic.png", "./".repeat(i))).collect();
    let handles: Vec<String> = (0..30).map(|i| format!("{:X}", 0x90 + i)).collect();
    let mut body = String::new();
    for (i, h) in handles.iter().enumerate() {
        body += &image(h, [i as f64 * 5.0, 0.0], [1.0, 0.0], [0.0, 1.0], [2.0, 2.0], 3, "");
    }
    let defs: Vec<(&str, &str)> = handles.iter().map(String::as_str).zip(names.iter().map(String::as_str)).collect();
    let d = load_in(dir.path(), &dxf(&body, &defs));
    let before = crate::raster::DECODES.with(|c| c.get());
    let sc = scene(&d);
    let decoded = crate::raster::DECODES.with(|c| c.get()) - before;
    assert_eq!(sc.images.len(), 30, "every IMAGE entity shows its picture");
    assert_eq!(decoded, 1, "one file is one decode, whatever its names look like");
}

#[test]
fn a_picture_is_refused_by_its_header_before_it_is_decoded() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_png(&dir.path().join("a.png"), 10, 10, four_colours);
    let before = crate::raster::DECODES.with(|c| c.get());
    assert!(crate::raster::load_file_within(&dir.path().join("a.png"), 99).is_none());
    assert_eq!(crate::raster::DECODES.with(|c| c.get()), before, "no pixel was decoded");
    let r = crate::raster::load_file_within(&dir.path().join("a.png"), 100).expect("100 pixels fit");
    assert_eq!((r.width, r.height), (10, 10));
    assert_eq!(crate::raster::DECODES.with(|c| c.get()), before + 1);
}
