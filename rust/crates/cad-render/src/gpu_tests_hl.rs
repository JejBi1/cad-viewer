//! Headless GPU tests of the highlight of a picked entity in the 2D view, of the batch mask, and of the buffers that are
//! written for each frame: a size the device refuses is a warning, never the panic that took the window down when a
//! block reference of 9 000 texts was clicked ("Buffer size 1073741824 is greater than the maximum buffer size").
use cad_types::{DrawBatch, Layer, Scene2d};
use eframe::wgpu;

use crate::capture::Rgba8;
use crate::gpu::GpuContext;
use crate::gpu_tests::gpu;
use crate::gpubuf::GPU_DYNAMIC_WARNING;
use crate::overlay2d::Overlays2d;
use crate::renderer2d::Renderer2d;
use crate::renderer3d::DynBuf;

const N: u32 = 160;
const ORANGE: [u8; 4] = [255, 140, 0, 255];

fn batch(layer: u32, color: [u8; 4], width: f32, lines: Vec<[f32; 2]>) -> DrawBatch {
    DrawBatch { layer, color, width, lines, fills: vec![] }
}

/// A grey line across the middle (layer 0) and a blue one above it (layer 1); the scene is 100 x 100.
fn scene() -> Scene2d {
    Scene2d {
        layers: (0..2).map(|i| Layer { id: i, name: format!("l{i}"), entities: 1, visible: true, off_in_file: false }).collect(),
        batches: vec![
            batch(0, [120, 120, 120, 255], 3.0, vec![[0.0, 50.0], [100.0, 50.0]]),
            batch(1, [0, 0, 255, 255], 3.0, vec![[0.0, 90.0], [100.0, 90.0]]),
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

fn shot(r: &mut Renderer2d) -> Rgba8 {
    r.capture_rgba(N, N, false).expect("capture")
}

fn px(r: &Renderer2d, p: [f64; 2]) -> (u32, u32) {
    let s = r.view.to_screen(p, r.viewport());
    (s[0].round() as u32, s[1].round() as u32)
}

/// Orange segments of a highlight: vertical bars at x = 10, 30, 50, 70 and 90, each 20 long, around the grey line.
fn bars() -> Vec<DrawBatch> {
    let lines: Vec<[f32; 2]> = (0..5).flat_map(|i| [[10.0 + 20.0 * i as f32, 30.0], [10.0 + 20.0 * i as f32, 50.0]]).collect();
    vec![batch(0, ORANGE, 3.0, lines)]
}

#[test]
fn a_highlight_is_made_once_cut_into_buffers_and_drawn_above_the_drawing() {
    let Some(g) = gpu("2d highlight") else { return };
    let mut r = renderer(&g);
    let before = shot(&mut r);
    let (x, y) = px(&r, [30.0, 40.0]);
    assert_eq!(before.pixel(x, y), [255, 255, 255, 255], "nothing is highlighted yet");
    r.set_highlight(&bars());
    assert_eq!(r.highlight_counts(), (5, 1), "five segments in one buffer");
    let whole = shot(&mut r);
    for i in 0..5 {
        let (x, y) = px(&r, [10.0 + 20.0 * f64::from(i), 40.0]);
        assert_eq!(whole.pixel(x, y), ORANGE, "bar {i}");
    }
    // the highlight is above the drawing (the grey line ends under the top of the bar at y = 50)
    let (x, y) = px(&r, [30.0, 50.0]);
    assert_eq!(whole.pixel(x, y), ORANGE);
    // nothing about it is uploaded again for a frame: the overlay buffers stay as they are (empty)
    assert_eq!(r.overlay_bytes(), 0);
    // buffers of two segments: the same picture, in three buffers
    r.set_buffer_chunk_bytes(Some(48));
    r.set_highlight(&bars());
    assert_eq!(r.highlight_counts(), (5, 3));
    assert_eq!(shot(&mut r).data, whole.data, "cutting the highlight into buffers changes nothing");
    assert!(r.take_warning().is_none());
    // it goes away with the pick, and with the scene
    r.clear_highlight();
    assert_eq!(r.highlight_counts(), (0, 0));
    assert_eq!(shot(&mut r).data, before.data);
    r.set_highlight(&bars());
    r.set_scene(&scene());
    assert_eq!(r.highlight_counts(), (0, 0), "a new scene has no highlight");
}

#[test]
fn the_pen_of_the_highlight_follows_the_device_pixel_ratio() {
    let Some(g) = gpu("2d highlight dpr") else { return };
    let mut r = renderer(&g);
    r.set_highlight(&[batch(0, ORANGE, 2.0, vec![[50.0, 20.0], [50.0, 80.0]])]);
    let wide = |r: &mut Renderer2d, dpr: f64| {
        let img = r.capture_rgba_dpr(N, N, dpr).expect("capture");
        let (_, y) = px(r, [50.0, 30.0]);
        (0..N).filter(|&x| img.pixel(x, y) == ORANGE).count()
    };
    let one = wide(&mut r, 1.0);
    let two = wide(&mut r, 2.0);
    assert!((1..=3).contains(&one), "2 px pen at dpr 1: {one}");
    assert!((3..=5).contains(&two), "4 px pen at dpr 2: {two}");
}

#[test]
fn a_device_that_refuses_buffers_gives_a_warning_for_the_highlight_and_the_overlays_not_a_panic() {
    let Some(g) = gpu("2d dynamic buffers refused") else { return };
    let mut r = renderer(&g);
    r.set_simulated_buffer_limit(Some(16));
    // the highlight of an entity that is far too big for the device
    r.set_highlight(&bars());
    assert_eq!(r.take_warning().as_deref(), Some(GPU_DYNAMIC_WARNING));
    // overlays are made for each frame: the frame is drawn without them
    let mut o = Overlays2d::default();
    o.measure_line([0.0, 0.0], [100.0, 100.0], false);
    o.measure_line([0.0, 100.0], [100.0, 0.0], true);
    r.set_overlays(o.clone());
    let img = shot(&mut r);
    let (x, y) = px(&r, [25.0, 25.0]);
    assert_eq!(img.pixel(x, y), [255, 255, 255, 255], "the measuring line did not fit: not drawn");
    assert_eq!(r.take_warning().as_deref(), Some(GPU_DYNAMIC_WARNING), "said once");
    shot(&mut r);
    assert!(r.take_warning().is_none(), "and not again for every frame");
    // a device that takes them again: the overlays are back
    r.set_simulated_buffer_limit(None);
    r.set_overlays(o);
    let img = shot(&mut r);
    assert_ne!(img.pixel(x, y), [255, 255, 255, 255], "the measuring line is drawn");
    assert!(r.take_warning().is_none());
}

#[test]
fn a_dynamic_buffer_over_the_limit_of_the_device_is_not_created_and_the_next_one_is() {
    let Some(g) = gpu("dynamic buffer over the limit") else { return };
    let limit = g.device.limits().max_buffer_size;
    let mut b = DynBuf::new("test", wgpu::BufferUsages::VERTEX);
    // 600 MB is what 9 000 texts asked for as a highlight (1 GiB as a power of two): the device takes 256 MiB at most
    let too_big = vec![0u8; (limit + 1) as usize];
    assert!(!b.upload(&g.device, &g.queue, &too_big));
    assert!(b.failed && b.buf.is_none() && b.len == 0);
    assert!(b.take_failure(), "reported");
    assert!(!b.take_failure(), "once");
    assert!(b.upload(&g.device, &g.queue, &[1, 2, 3, 4]));
    assert!(!b.failed && b.buf.is_some() && b.len == 4);
    // a size that is not a power of two is rounded up, but never past what the device takes
    let big = vec![0u8; (limit - 1000) as usize];
    assert!(b.upload(&g.device, &g.queue, &big), "just under the limit");
    drop(big);
    assert!(!b.upload(&g.device, &g.queue, &too_big));
    assert!(b.buf.is_none(), "a failed upload leaves nothing stale to draw");
}

#[test]
fn a_batch_mask_hides_batches_without_a_new_scene_and_the_bounds_follow_it() {
    let Some(g) = gpu("2d batch mask") else { return };
    let mut r = renderer(&g);
    let (x, y) = px(&r, [50.0, 90.0]);
    assert_eq!(shot(&mut r).pixel(x, y), [0, 0, 255, 255]);
    assert_eq!(r.visible_bounds(), Some(([0.0, 50.0], [100.0, 90.0])));
    r.set_batch_mask(&[true, false]);
    assert_eq!(shot(&mut r).pixel(x, y), [255, 255, 255, 255], "the second batch is not drawn");
    assert_eq!(r.visible_bounds(), Some(([0.0, 50.0], [100.0, 50.0])));
    // a mask of the wrong length is ignored; a layer that is off hides its batch as well
    r.set_batch_mask(&[false]);
    assert_eq!(shot(&mut r).pixel(x, y), [0, 0, 255, 255]);
    r.set_batch_mask(&[true, true]);
    r.set_layer_visible(1, false);
    assert_eq!(shot(&mut r).pixel(x, y), [255, 255, 255, 255]);
    r.set_layer_visible(1, true);
    r.set_batch_mask(&[false, false]);
    assert_eq!(r.visible_bounds(), Some(([0.0, 0.0], [100.0, 100.0])), "nothing shown: the bounds of the scene");
    // a new scene starts with every batch shown
    r.set_scene(&scene());
    assert_eq!(shot(&mut r).pixel(x, y), [0, 0, 255, 255]);
}

#[test]
fn a_scene_that_is_handed_over_is_drawn_like_a_borrowed_one_and_fitted_to_what_is_shown() {
    let Some(g) = gpu("2d owned scene") else { return };
    let mut borrowed = renderer(&g);
    borrowed.set_batch_mask(&[true, false]);
    borrowed.fit();
    let mut owned = Renderer2d::new(&g.device, wgpu::TextureFormat::Rgba8Unorm).with_queue(&g.queue);
    owned.set_viewport(N, N, 1.0);
    owned.set_light_background(true);
    // the batches are given up; the mask is in place before the view is fitted
    owned.set_scene_owned(scene(), &[true, false], &[], None, false);
    assert_eq!(owned.visible_bounds(), borrowed.visible_bounds());
    assert_eq!(owned.view.scale, borrowed.view.scale);
    assert_eq!(owned.view.center, borrowed.view.center);
    assert_eq!(shot(&mut owned).data, shot(&mut borrowed).data);
    // keeping the view: the new scene is there, pan and zoom are not touched
    owned.view.center = [10.0, 20.0];
    owned.view.scale = 3.0;
    owned.set_scene_owned(scene(), &[true, true], &[], None, true);
    assert_eq!((owned.view.center, owned.view.scale), ([10.0, 20.0], 3.0));
    assert_eq!(owned.visible_bounds(), Some(([0.0, 50.0], [100.0, 90.0])));
    // a mask of the wrong length is ignored
    owned.set_scene_owned(scene(), &[true], &[], None, true);
    assert_eq!(owned.visible_bounds(), Some(([0.0, 50.0], [100.0, 90.0])));
}

/// Four horizontal pens of 3 pixels, 20 units apart: foreground (baked as light grey), the same white that is an explicit colour,
/// a colour that is the first candidate for a marker, and the background (baked as dark blue).
fn pens_scene() -> Scene2d {
    let pen = |y: f32, color: [u8; 4]| batch(0, color, 3.0, vec![[0.0, y], [100.0, y]]);
    Scene2d {
        layers: vec![Layer { id: 0, name: "0".into(), entities: 1, visible: true, off_in_file: false }],
        batches: vec![
            pen(20.0, [200, 200, 200, 255]),
            pen(40.0, [255, 255, 255, 255]),
            pen(60.0, [0, 0, 1, 255]),
            pen(80.0, [0x21, 0x28, 0x30, 255]),
        ],
        bounds: Some(([0.0, 0.0], [100.0, 100.0])),
        layout: "Model".into(),
        layouts: vec!["Model".into()],
        ..Default::default()
    }
}

#[test]
fn foreground_and_background_batches_follow_the_palette_and_no_other_colour_does() {
    let Some(g) = gpu("2d palette") else { return };
    let mut r = Renderer2d::new(&g.device, wgpu::TextureFormat::Rgba8Unorm).with_queue(&g.queue);
    r.set_viewport(N, N, 1.0);
    const DARK: [u8; 3] = [0x21, 0x28, 0x30];
    r.set_scene_owned(pens_scene(), &[true; 4], &[1, 0, 0, 2], Some(([255, 255, 255], DARK)), false);
    let at = |r: &Renderer2d, y: f64| px(r, [50.0, y]);
    let (a, b, c, d) = (at(&r, 20.0), at(&r, 40.0), at(&r, 60.0), at(&r, 80.0));
    let img = shot(&mut r);
    assert_eq!(img.pixel(2, 2)[..3], DARK, "the background of the palette");
    assert_eq!(img.pixel(a.0, a.1), [255, 255, 255, 255], "the foreground batch is drawn in the foreground colour, not in the grey it was made with");
    assert_eq!(img.pixel(b.0, b.1), [255, 255, 255, 255]);
    assert_eq!(img.pixel(c.0, c.1), [0, 0, 1, 255], "a colour like a marker is not mistaken for one");
    assert_eq!(img.pixel(d.0, d.1)[..3], DARK, "the background batch (a wipeout) is the background");
    // white paper: only the foreground and the background change
    r.set_palette([0, 0, 0], [255, 255, 255]);
    let img = shot(&mut r);
    assert_eq!(img.pixel(2, 2), [255, 255, 255, 255]);
    assert_eq!(img.pixel(a.0, a.1), [0, 0, 0, 255], "black pen on white paper");
    assert_eq!(img.pixel(b.0, b.1), [255, 255, 255, 255], "an explicit white stays white: invisible on white, as in the drawing");
    assert_eq!(img.pixel(c.0, c.1), [0, 0, 1, 255]);
    assert_eq!(img.pixel(d.0, d.1), [255, 255, 255, 255], "the wipeout is white");
    // back
    r.set_palette([255, 255, 255], DARK);
    assert_eq!(shot(&mut r).pixel(a.0, a.1), [255, 255, 255, 255]);
    // the transparency of a pen is kept
    let mut s = pens_scene();
    s.batches[0].color[3] = 128;
    r.set_scene_owned(s, &[true; 4], &[1, 0, 0, 2], Some(([255, 255, 255], DARK)), false);
    r.set_palette([0, 0, 0], [255, 255, 255]);
    let p = shot(&mut r).pixel(a.0, a.1);
    assert!(p[0] > 100 && p[0] < 160 && p[0] == p[1] && p[1] == p[2], "half black on white: {p:?}");
    // a scene that is borrowed has no roles at all, whatever the palette says
    r.set_scene(&pens_scene());
    r.set_palette([0, 0, 0], [255, 255, 255]);
    assert_eq!(shot(&mut r).pixel(a.0, a.1)[..3], [200, 200, 200]);
}
