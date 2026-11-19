//! Headless GPU tests of the 2D renderer (see `gpu_tests.rs` for the skip rule).
use cad_types::{DrawBatch, Layer, Scene2d};
use eframe::wgpu;

use crate::capture::Rgba8;
use crate::gpu::GpuContext;
use crate::gpu_tests::gpu;
use crate::overlay2d::Overlays2d;
use crate::renderer2d::{LineWidthMode, Renderer2d};

const N: u32 = 200;

fn square(layer: u32, x0: f32, y0: f32, x1: f32, y1: f32, color: [u8; 4]) -> DrawBatch {
    DrawBatch {
        layer,
        color,
        width: 0.0,
        lines: vec![],
        fills: vec![[x0, y0], [x1, y0], [x1, y1], [x0, y0], [x1, y1], [x0, y1]],
    }
}

fn line(layer: u32, a: [f32; 2], b: [f32; 2], width: f32, color: [u8; 4]) -> DrawBatch {
    DrawBatch { layer, color, width, lines: vec![a, b], fills: vec![] }
}

fn scene() -> Scene2d {
    Scene2d {
        layers: vec![
            Layer { id: 0, name: "0".into(), entities: 1, visible: true, off_in_file: false },
            Layer { id: 1, name: "lines".into(), entities: 1, visible: true, off_in_file: false },
            Layer { id: 2, name: "hidden".into(), entities: 1, visible: false, off_in_file: true },
        ],
        batches: vec![
            square(0, 0.0, 0.0, 100.0, 100.0, [255, 0, 0, 255]),
            line(1, [0.0, 0.0], [100.0, 100.0], 5.0, [0, 0, 255, 255]),
            line(2, [0.0, 100.0], [100.0, 0.0], 5.0, [0, 255, 0, 255]),
        ],
        bounds: Some(([0.0, 0.0], [100.0, 100.0])),
        layout: "Model".into(),
        layouts: vec!["Model".into()],
        ..Default::default()
    }
}

fn renderer(g: &GpuContext) -> Renderer2d {
    let mut r = Renderer2d::new(&g.device, wgpu::TextureFormat::Rgba8Unorm).with_queue(&g.queue);
    r.set_viewport(N, N, 1.0);
    r.set_scene(&scene());
    r.set_light_background(true);
    r
}

fn px(r: &Renderer2d, p: [f64; 2]) -> (u32, u32) {
    let s = r.view.to_screen(p, r.viewport());
    (s[0].round() as u32, s[1].round() as u32)
}

fn pixel(r: &mut Renderer2d) -> Rgba8 {
    r.capture_rgba(N, N, false).expect("capture")
}

#[test]
fn fills_lines_background_and_layers() {
    let Some(g) = gpu("2d fills_lines_background_and_layers") else { return };
    let mut r = renderer(&g);
    // the drawing was fitted with a 3 % margin
    assert!((r.view.scale - f64::from(N) / 106.0).abs() < 1e-9, "{}", r.view.scale);
    let img = pixel(&mut r);
    assert_eq!(img.pixel(2, 2), [255, 255, 255, 255], "white background");
    let (x, y) = px(&r, [75.0, 25.0]);
    assert_eq!(img.pixel(x, y), [255, 0, 0, 255], "filled square");
    let (x, y) = px(&r, [50.0, 50.0]);
    assert_eq!(img.pixel(x, y), [0, 0, 255, 255], "the line is drawn above the fill (z-order)");
    // the green diagonal of the hidden layer (x + y = 100) is not drawn: (75, 25) is on it and shows the fill
    let (x, y) = px(&r, [75.0, 25.0]);
    assert_ne!(img.pixel(x, y)[1], 255);
    // dark default background
    r.set_light_background(false);
    assert_eq!(pixel(&mut r).pixel(2, 2), [0x21, 0x28, 0x30, 255]);
    r.set_light_background(true);
    // layers: hide the fill, show the hidden layer
    r.set_layer_visible(0, false);
    r.set_layer_visible(2, true);
    let img = pixel(&mut r);
    let (x, y) = px(&r, [85.0, 5.0]);
    assert_eq!(img.pixel(x, y), [255, 255, 255, 255], "layer 0 hidden");
    let (x, y) = px(&r, [75.0, 25.0]);
    assert_eq!(img.pixel(x, y), [0, 255, 0, 255], "layer 2 shown");
    assert!(r.layer_visible(2) && !r.layer_visible(0));
    // fitting follows the visible layers
    r.set_layer_visible(1, false);
    r.set_layer_visible(2, false);
    r.set_layer_visible(0, true);
    let b = r.visible_bounds().expect("bounds");
    assert_eq!(b, ([0.0, 0.0], [100.0, 100.0]));
}

#[test]
fn line_width_scales_with_the_zoom_and_hairlines_stay_visible() {
    let Some(g) = gpu("2d line_width_scales_with_the_zoom") else { return };
    let mut r = renderer(&g);
    r.width_mode = LineWidthMode::DrawingUnits;
    r.set_layer_visible(0, false);
    let across = |r: &mut Renderer2d| -> u32 {
        // count blue pixels along the horizontal line through the diagonal's centre
        let img = pixel(r);
        let (_, y) = px(r, [50.0, 50.0]);
        (0..N).filter(|&x| img.pixel(x, y)[0] < 200).count() as u32
    };
    let wide = across(&mut r);
    // 5 units * 1.887 px = 9.4 px perpendicular, 13.3 px horizontally along the 45 degree diagonal
    assert!((11..=16).contains(&wide), "{wide}");
    let vp = r.viewport();
    r.view.zoom_at([100.0, 100.0], 0.25, vp);
    let narrow = across(&mut r);
    assert!(narrow < wide && narrow >= 1, "{narrow}");
    // pixel width mode: constant regardless of the zoom
    r.width_mode = LineWidthMode::ScreenPixels;
    let a = across(&mut r);
    let vp = r.viewport();
    r.view.zoom_at([100.0, 100.0], 2.0, vp);
    let b = across(&mut r);
    assert_eq!(a, b);
    // hairline: width 0 still draws something
    let mut s = scene();
    s.batches = vec![line(1, [0.0, 0.0], [100.0, 100.0], 0.0, [0, 0, 255, 255])];
    r.width_mode = LineWidthMode::DrawingUnits;
    r.set_scene(&s);
    assert!(across(&mut r) >= 1);
}

#[test]
fn zoom_and_pan_move_the_picture() {
    let Some(g) = gpu("2d zoom_and_pan_move_the_picture") else { return };
    let mut r = renderer(&g);
    let before = pixel(&mut r);
    r.view.pan_pixels(40.0, 0.0);
    let after = pixel(&mut r);
    assert_ne!(before, after);
    // the content moved right: the left edge of the red square is now more than 40 px from the left
    let (x, y) = px(&r, [1.0, 50.0]);
    assert!(x > 40, "{x}");
    assert_eq!(after.pixel(x + 2, y)[1], 0);
    // keep the view when the scene is replaced
    let v = r.view;
    r.set_scene_keep_view(&scene());
    assert_eq!(r.view, v);
    r.set_scene(&scene());
    assert_ne!(r.view, v);
}

#[test]
fn overlays_are_drawn_on_top() {
    let Some(g) = gpu("2d overlays_are_drawn_on_top") else { return };
    let mut r = renderer(&g);
    r.set_layer_visible(0, false);
    r.set_layer_visible(1, false);
    let base = pixel(&mut r);
    let mut o = Overlays2d::default();
    o.selection(vec![[10.0, 20.0], [90.0, 20.0]], 4.0);
    o.measure_dot([50.0, 80.0]);
    o.snap_square([20.0, 60.0]);
    o.measure_line([10.0, 90.0], [90.0, 90.0], true);
    o.measure_text([30.0, 70.0], "12,50");
    r.set_overlays(o);
    let img = pixel(&mut r);
    let (x, y) = px(&r, [50.0, 20.0]);
    assert_eq!(img.pixel(x, y), [255, 140, 0, 255], "selection highlight (255, 140, 0)");
    let (x, y) = px(&r, [50.0, 80.0]);
    assert_eq!(img.pixel(x, y), [230, 30, 30, 255], "measurement dot");
    let (x, y) = px(&r, [20.0, 60.0]);
    assert_eq!(img.pixel(x, y), base.pixel(x, y), "the square is an outline: its centre stays empty");
    // right side of the 10 px square: 5 px from the centre, pen colour (0, 200, 255)
    let side = img.pixel(x + 5, y);
    assert!(side[0] < 60 && side[2] > 200, "outline pixel {side:?}");
    // dashed line has gaps
    let (_, y) = px(&r, [0.0, 90.0]);
    let (xa, _) = px(&r, [12.0, 90.0]);
    let (xb, _) = px(&r, [88.0, 90.0]);
    let red = (xa..xb).filter(|&x| img.pixel(x, y)[0] > 200 && img.pixel(x, y)[1] < 180).count();
    assert!(red > 10 && red < (xb - xa) as usize - 10, "dashes: {red} of {}", xb - xa);
    // text pixels exist near (30, 70)
    let (tx, ty) = px(&r, [30.0, 70.0]);
    let ink = (ty..ty + 20).any(|yy| (tx..tx + 50).any(|xx| img.pixel(xx, yy)[1] < 100 && img.pixel(xx, yy)[0] > 150));
    assert!(ink, "label text drawn");
    r.set_overlays(Overlays2d::default());
    assert_eq!(pixel(&mut r), base);
}

#[test]
fn thumbnail_has_paper_border_and_transparent_surroundings() {
    let Some(g) = gpu("2d thumbnail_has_paper_border") else { return };
    let mut r = renderer(&g);
    let t = r.thumbnail_rgba(100).expect("thumbnail");
    assert_eq!((t.width, t.height), (100, 100));
    // the drawing is square: the paper covers the whole canvas (the 3 % margin is part of the paper)
    assert_eq!(t.pixel(0, 0)[3], 255, "paper corner: opaque");
    assert_eq!(t.pixel(0, 0)[..3], [150, 150, 150], "grey border");
    assert_eq!(t.pixel(1, 1), [255, 255, 255, 255], "white paper in the margin");
    // content: the red square (world (75, 25) is right and low)
    assert_eq!(t.pixel(75, 75), [255, 0, 0, 255]);
    // a wide drawing leaves transparent bands above and below the paper
    let mut s = scene();
    s.batches = vec![line(1, [0.0, 0.0], [400.0, 20.0], 2.0, [0, 0, 0, 255])];
    s.bounds = Some(([0.0, 0.0], [400.0, 20.0]));
    r.set_scene(&s);
    let t = r.thumbnail_rgba(100).expect("thumbnail");
    assert_eq!(t.pixel(50, 2)[3], 0, "transparent above the paper");
    assert_eq!(t.pixel(50, 50)[3], 255);
    // empty drawing: no thumbnail
    let mut empty = Renderer2d::new(&g.device, wgpu::TextureFormat::Rgba8Unorm).with_queue(&g.queue);
    empty.set_viewport(N, N, 1.0);
    assert!(empty.thumbnail_rgba(64).is_err());
    let png = r.thumbnail_png(64).expect("png");
    assert_eq!(image::load_from_memory(&png).expect("decode").to_rgba8().dimensions(), (64, 64));
}

#[test]
fn translucent_fills_blend() {
    let Some(g) = gpu("2d translucent_fills_blend") else { return };
    let mut r = renderer(&g);
    let mut s = scene();
    s.batches = vec![square(0, 0.0, 0.0, 100.0, 100.0, [0, 0, 0, 128])];
    r.set_scene(&s);
    let img = pixel(&mut r);
    let (x, y) = px(&r, [50.0, 50.0]);
    // 50 % black over white
    let c = img.pixel(x, y);
    assert!((i32::from(c[0]) - 127).abs() <= 2, "{c:?}");
}

#[test]
fn capture_errors_and_missing_queue() {
    let Some(g) = gpu("2d capture_errors_and_missing_queue") else { return };
    let mut r = renderer(&g);
    assert!(r.capture_rgba(0, 10, false).is_err());
    assert!(r.capture_png(100_000, 10, false).is_err());
    let mut nq = Renderer2d::new(&g.device, wgpu::TextureFormat::Rgba8Unorm);
    assert!(nq.capture_rgba(10, 10, false).is_err());
    let png = r.capture_png(64, 48, true).expect("png");
    assert_eq!(image::load_from_memory(&png).expect("decode").to_rgba8().dimensions(), (64, 48));
}

#[test]
fn dump_2d_overlays() {
    let Ok(dir) = std::env::var("CAD_RENDER_DUMP") else { return };
    let Some(g) = gpu("dump_2d_overlays") else { return };
    let mut r = renderer(&g);
    r.set_layer_visible(0, false);
    let mut o = Overlays2d::default();
    o.selection(vec![[10.0, 20.0], [90.0, 20.0]], 4.0);
    o.measure_dot([50.0, 80.0]);
    o.snap_square([20.0, 60.0]);
    o.measure_line([10.0, 90.0], [90.0, 90.0], true);
    o.measure_text([30.0, 70.0], "12,50");
    r.set_overlays(o);
    let png = r.capture_png(N, N, false).expect("png");
    std::fs::write(std::path::Path::new(&dir).join("overlay2d.png"), png).expect("write");
    let t = r.thumbnail_png(128).expect("png");
    std::fs::write(std::path::Path::new(&dir).join("thumb2d.png"), t).expect("write");
}

// ------------------------------------------------------------------------------------------------ pictures, dots, origin

fn dxf_tag(code: i32, v: impl std::fmt::Display) -> String {
    format!("{code:>3}\n{v}\n")
}

/// Minimal DXF: an entity section and the IMAGEDEF objects given as (handle, file name) pairs.
fn dxf_text(entities: &str, defs: &[(&str, &str)]) -> String {
    let mut s = String::from(
        "  0\nSECTION\n  2\nHEADER\n  9\n$ACADVER\n  1\nAC1027\n  9\n$INSUNITS\n 70\n4\n  0\nENDSEC\n  0\nSECTION\n  2\nTABLES\n  0\nTABLE\n  2\nLTYPE\n 70\n1\n  0\nLTYPE\n  2\nContinuous\n 70\n0\n  3\nSolid line\n 72\n65\n 73\n0\n 40\n0\n  0\nENDTAB\n  0\nTABLE\n  2\nLAYER\n 70\n1\n  0\nLAYER\n  2\n0\n 70\n0\n 62\n7\n  6\nContinuous\n  0\nENDTAB\n  0\nTABLE\n  2\nSTYLE\n 70\n1\n  0\nSTYLE\n  2\nStandard\n 70\n0\n 40\n0\n 41\n1\n 50\n0\n  3\narial.ttf\n  0\nENDTAB\n  0\nENDSEC\n  0\nSECTION\n  2\nBLOCKS\n  0\nENDSEC\n  0\nSECTION\n  2\nENTITIES\n",
    );
    s.push_str(entities);
    s.push_str("  0\nENDSEC\n  0\nSECTION\n  2\nOBJECTS\n");
    for (handle, name) in defs {
        s.push_str(&dxf_tag(0, "IMAGEDEF"));
        s.push_str(&dxf_tag(5, handle));
        s.push_str(&dxf_tag(1, name));
    }
    s.push_str("  0\nENDSEC\n  0\nEOF\n");
    s
}

/// IMAGE: insert point, per-pixel vectors, size in pixels, IMAGEDEF handle and flags (1 = shown).
fn dxf_image(def: &str, insert: [f64; 2], u: [f64; 2], v: [f64; 2], size: [f64; 2], flags: i32) -> String {
    let mut s = dxf_tag(0, "IMAGE") + &dxf_tag(8, "0") + &dxf_tag(10, insert[0]) + &dxf_tag(20, insert[1]) + &dxf_tag(30, 0);
    s += &(dxf_tag(11, u[0]) + &dxf_tag(21, u[1]) + &dxf_tag(31, 0) + &dxf_tag(12, v[0]) + &dxf_tag(22, v[1]) + &dxf_tag(32, 0));
    s += &(dxf_tag(13, size[0]) + &dxf_tag(23, size[1]) + &dxf_tag(340, def) + &dxf_tag(70, flags) + &dxf_tag(71, 1));
    s
}

/// SOLID rectangle filled with the ACI colour `aci`.
fn dxf_solid(x0: f64, y0: f64, x1: f64, y1: f64, aci: i32) -> String {
    dxf_tag(0, "SOLID")
        + &dxf_tag(8, "0")
        + &dxf_tag(62, aci)
        + &dxf_tag(10, x0)
        + &dxf_tag(20, y0)
        + &dxf_tag(11, x1)
        + &dxf_tag(21, y0)
        + &dxf_tag(12, x0)
        + &dxf_tag(22, y1)
        + &dxf_tag(13, x1)
        + &dxf_tag(23, y1)
}

/// Channel-wise closeness: bilinear filtering and anti-aliased edges are not exact.
fn close(c: [u8; 4], want: [u8; 4], tol: i32) -> bool {
    c.iter().zip(want.iter()).all(|(a, b)| (i32::from(*a) - i32::from(*b)).abs() <= tol)
}

#[test]
fn pictures_are_drawn_from_the_file_in_z_order() {
    let Some(g) = gpu("2d pictures_are_drawn_from_the_file_in_z_order") else { return };
    let dir = tempfile::tempdir().expect("tempdir");
    // top row: red, green; bottom row: transparent, grey
    let colours = |x: u32, y: u32| match (x, y) {
        (0, 0) => [255, 0, 0, 255],
        (1, 0) => [0, 255, 0, 255],
        (0, 1) => [0, 0, 0, 0],
        _ => [128, 128, 128, 255],
    };
    image::RgbaImage::from_fn(2, 2, |x, y| image::Rgba(colours(x, y)))
        .save_with_format(dir.path().join("pic.png"), image::ImageFormat::Png)
        .expect("png");
    // a blue SOLID before the picture (seen through its transparent quarter), a small magenta one after it (above it);
    // flags 3 | 8: shown, with transparency (without bit 8 the alpha of the picture is 255, as in ezdxf)
    let entities = dxf_solid(0.0, 0.0, 20.0, 20.0, 5)
        + &dxf_image("90", [0.0, 0.0], [10.0, 0.0], [0.0, 10.0], [2.0, 2.0], 3 | 8)
        + &dxf_solid(11.0, 11.0, 12.5, 12.5, 6);
    let path = dir.path().join("plan.dxf");
    std::fs::write(&path, dxf_text(&entities, &[("90", "pic.png")])).expect("dxf");
    let drawing = cad_2d::load(&path, &cad_2d::LoadOptions::default(), &mut |_| {}).expect("drawing loads");
    let sc = drawing.scene_with("Model", &cad_2d::SceneOptions { light_bg: true, ..Default::default() }).expect("scene");
    assert_eq!(sc.images.len(), 1, "the picture file is found next to the drawing");

    let mut r = Renderer2d::new(&g.device, wgpu::TextureFormat::Rgba8Unorm).with_queue(&g.queue);
    r.set_viewport(N, N, 1.0);
    r.set_light_background(true);
    r.set_scene(&sc);
    let img = pixel(&mut r);
    // texel centres: the picture is 20 x 20 drawing units, each texel 10 units
    let (x, y) = px(&r, [5.0, 15.0]);
    assert!(close(img.pixel(x, y), [255, 0, 0, 255], 16), "top left texel: {:?}", img.pixel(x, y));
    let (x, y) = px(&r, [15.0, 15.0]);
    assert!(close(img.pixel(x, y), [0, 255, 0, 255], 16), "top right texel: {:?}", img.pixel(x, y));
    let (x, y) = px(&r, [5.0, 5.0]);
    assert!(close(img.pixel(x, y), [0, 0, 255, 255], 16), "transparent texel shows the SOLID drawn before the picture: {:?}", img.pixel(x, y));
    let (x, y) = px(&r, [15.0, 5.0]);
    assert!(close(img.pixel(x, y), [128, 128, 128, 255], 16), "grey texel covers the SOLID drawn before it: {:?}", img.pixel(x, y));
    let (x, y) = px(&r, [11.75, 11.75]);
    assert_eq!(img.pixel(x, y), [255, 0, 255, 255], "the SOLID drawn after the picture is above it");
    // layers: the picture is on layer 0 with the SOLIDs; with the layer off the background shows
    r.set_layer_visible(0, false);
    let img = pixel(&mut r);
    let (x, y) = px(&r, [5.0, 15.0]);
    assert_eq!(img.pixel(x, y), [255, 255, 255, 255], "hidden layer draws nothing");
}

#[test]
fn a_line_with_two_equal_points_is_a_dot_of_the_pen_width() {
    let Some(g) = gpu("2d a_line_with_two_equal_points_is_a_dot") else { return };
    let mut r = renderer(&g);
    assert_eq!(LineWidthMode::default(), LineWidthMode::ScreenPixels, "the default is the cosmetic pen of the Qt viewer");
    let mut s = scene();
    s.batches = vec![line(1, [50.0, 50.0], [50.0, 50.0], 6.0, [0, 0, 255, 255])];
    r.set_scene(&s);
    // a single point has no extent to fit: 2 px per unit around it
    r.view = crate::view2d::View2d { center: [50.0, 50.0], scale: 2.0 };
    let img = pixel(&mut r);
    let (x, y) = px(&r, [50.0, 50.0]);
    assert_eq!(img.pixel(x, y), [0, 0, 255, 255], "the dot is drawn at its centre");
    assert!(close(img.pixel(x + 2, y), [0, 0, 255, 255], 40), "inside the 6 px diameter (edge pixels are anti-aliased): {:?}", img.pixel(x + 2, y));
    assert_eq!(img.pixel(x + 6, y), [255, 255, 255, 255], "outside it");
    assert_eq!(img.pixel(x, y + 6), [255, 255, 255, 255], "outside it, below");
}

#[test]
fn scene_origin_keeps_far_coordinates_exact() {
    let Some(g) = gpu("2d scene_origin_keeps_far_coordinates_exact") else { return };
    // the same relative geometry, once around an origin far away (1 000 km) and once around zero: the coordinates
    // are exact in both, so the pictures are the same
    let far = [1_000_000.0, 2_000_000.0];
    let batches = || vec![line(1, [-10.0, -10.0], [10.0, 10.0], 3.0, [0, 0, 255, 255]), square(1, -20.0, -20.0, -12.0, -12.0, [255, 0, 0, 255])];
    let rel_bounds = Some(([-20.0, -20.0], [10.0, 10.0]));
    let far_scene = Scene2d {
        batches: batches(),
        bounds: rel_bounds.map(|(a, b)| ([a[0] + far[0], a[1] + far[1]], [b[0] + far[0], b[1] + far[1]])),
        origin: far,
        ..scene()
    };
    let near_scene = Scene2d { batches: batches(), bounds: rel_bounds, origin: [0.0, 0.0], ..scene() };
    let mut a = Renderer2d::new(&g.device, wgpu::TextureFormat::Rgba8Unorm).with_queue(&g.queue);
    a.set_viewport(N, N, 1.0);
    a.set_light_background(true);
    a.set_scene(&far_scene);
    let mut b = Renderer2d::new(&g.device, wgpu::TextureFormat::Rgba8Unorm).with_queue(&g.queue);
    b.set_viewport(N, N, 1.0);
    b.set_light_background(true);
    b.set_scene(&near_scene);
    let (ia, ib) = (pixel(&mut a), pixel(&mut b));
    // the world origin of the drawing is where the line crosses the middle of the picture
    let (x, y) = px(&a, far);
    assert!(close(ia.pixel(x, y), [0, 0, 255, 255], 40), "the line passes through the origin: {:?}", ia.pixel(x, y));
    let (x, y) = px(&a, [far[0] - 16.0, far[1] - 16.0]);
    assert_eq!(ia.pixel(x, y), [255, 0, 0, 255], "the square away from the line");
    let worst = ia.data.iter().zip(&ib.data).map(|(p, q)| (i32::from(*p) - i32::from(*q)).abs()).max().unwrap_or(0);
    assert!(worst <= 1, "far and near origins draw the same picture (worst channel difference {worst})");
}

