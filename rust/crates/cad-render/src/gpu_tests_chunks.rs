//! Headless GPU tests of vertex data cut into several buffers (2D scenes and 3D meshes) and of a device that refuses
//! buffers: the picture is the one a single buffer gives, and a failure is a warning, not a panic.
use cad_types::{DrawBatch, EntityKind, EntityRef, Layer, Scene2d};
use eframe::wgpu;

use crate::camera::StdView;
use crate::capture::{CaptureOptions, Rgba8};
use crate::gpu::GpuContext;
use crate::gpu_tests::{cube_mesh, cube_tree, gpu, SIZE};
use crate::gpubuf::{try_buffer, try_buffer_limited, GPU_BUFFER_WARNING};
use crate::renderer2d::Renderer2d;
use crate::renderer3d::{Highlight, Renderer3d, Style};

const N: u32 = 160;

fn tri(a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> [[f32; 2]; 3] {
    [a, b, c]
}

/// Four layers, drawn in this order: a red square (layer 0), blue lines over it (layer 1), green and yellow triangles that
/// cover part of both (layer 2), thick magenta lines on top (layer 3). Several batches span many triangles / segments, so
/// that small buffers cut them.
fn scene() -> Scene2d {
    let mut red = Vec::new();
    for (x, y) in [(0.0f32, 0.0f32), (50.0, 0.0), (0.0, 50.0), (50.0, 50.0)] {
        red.extend(tri([x, y], [x + 50.0, y], [x + 50.0, y + 50.0]));
        red.extend(tri([x, y], [x + 50.0, y + 50.0], [x, y + 50.0]));
    }
    let blue: Vec<[f32; 2]> = (0..25).flat_map(|i| [[i as f32 * 4.0, 0.0], [i as f32 * 4.0 + 20.0, 100.0]]).collect();
    let mut green = Vec::new();
    for i in 0..6 {
        let o = i as f32 * 12.0;
        green.extend(tri([20.0 + o, 10.0], [50.0 + o, 10.0], [35.0 + o, 60.0]));
    }
    let yellow = tri([60.0, 60.0], [100.0, 60.0], [80.0, 100.0]).to_vec();
    let magenta: Vec<[f32; 2]> = (0..9).flat_map(|i| [[0.0, i as f32 * 12.0], [100.0, i as f32 * 12.0 + 6.0]]).collect();
    let batch = |layer: u32, color: [u8; 4], width: f32, lines: Vec<[f32; 2]>, fills: Vec<[f32; 2]>| DrawBatch { layer, color, width, lines, fills };
    Scene2d {
        layers: (0..4).map(|i| Layer { id: i, name: format!("l{i}"), entities: 1, visible: true, off_in_file: false }).collect(),
        batches: vec![
            batch(0, [255, 0, 0, 255], 0.0, vec![], red),
            batch(1, [0, 0, 255, 255], 2.0, blue, vec![]),
            batch(2, [0, 255, 0, 255], 0.0, vec![], green),
            batch(2, [255, 255, 0, 255], 0.0, vec![], yellow),
            batch(3, [255, 0, 255, 255], 3.0, magenta, vec![]),
        ],
        bounds: Some(([0.0, 0.0], [100.0, 100.0])),
        layout: "Model".into(),
        layouts: vec!["Model".into()],
        ..Default::default()
    }
}

fn renderer2d(g: &GpuContext, chunk: Option<u64>) -> Renderer2d {
    let mut r = Renderer2d::new(&g.device, wgpu::TextureFormat::Rgba8Unorm).with_queue(&g.queue);
    r.set_viewport(N, N, 1.0);
    r.set_buffer_chunk_bytes(chunk);
    r.set_scene(&scene());
    r.set_light_background(true);
    r
}

fn shot2d(r: &mut Renderer2d) -> Rgba8 {
    r.capture_rgba(N, N, false).expect("capture")
}

#[test]
fn a_scene_in_many_small_buffers_draws_the_picture_of_one_buffer() {
    let Some(g) = gpu("2d chunked buffers") else { return };
    let mut whole = renderer2d(&g, None);
    assert_eq!(whole.buffer_counts(), (1, 1));
    let mut views: Vec<(&str, Vec<u32>)> = vec![("all layers", vec![]), ("blue lines hidden", vec![1]), ("red and green hidden", vec![0, 2]), ("only the top", vec![0, 1, 2])];
    views.push(("nothing", vec![0, 1, 2, 3]));
    for chunk in [24u64, 36, 100, 600, 2_000] {
        let mut small = renderer2d(&g, Some(chunk));
        let (fills, lines) = small.buffer_counts();
        if chunk <= 100 {
            assert!(fills > 1 && lines > 1, "chunk {chunk}: {fills} fill and {lines} line buffers");
        }
        for (name, hidden) in &views {
            for r in [&mut whole, &mut small] {
                for l in 0..4 {
                    r.set_layer_visible(l, !hidden.contains(&l));
                }
            }
            let (a, b) = (shot2d(&mut whole), shot2d(&mut small));
            assert_eq!(a.data, b.data, "chunk {chunk} bytes, {name}: the picture differs");
        }
        assert_eq!(small.take_warning(), None);
    }
    // the picture is not empty: the red square, the yellow triangle and the magenta lines are there
    for l in 0..4 {
        whole.set_layer_visible(l, true);
    }
    let img = shot2d(&mut whole);
    let colours: std::collections::HashSet<[u8; 4]> = (0..N).flat_map(|y| (0..N).map(move |x| (x, y))).map(|(x, y)| img.pixel(x, y)).collect();
    for c in [[255, 0, 0, 255], [0, 255, 0, 255], [255, 255, 0, 255], [255, 0, 255, 255], [0, 0, 255, 255]] {
        assert!(colours.contains(&c), "{c:?} is not in the picture");
    }
}

#[test]
fn a_device_that_refuses_buffers_gives_a_warning_and_a_partial_picture_not_a_panic() {
    let Some(g) = gpu("2d refused buffers") else { return };
    let whole = {
        let mut r = renderer2d(&g, None);
        shot2d(&mut r)
    };
    // buffers of 240 bytes (10 segments, 18 fill corners) are refused, the smaller last ones are taken
    let mut r = Renderer2d::new(&g.device, wgpu::TextureFormat::Rgba8Unorm).with_queue(&g.queue);
    r.set_viewport(N, N, 1.0);
    r.set_buffer_chunk_bytes(Some(240));
    r.set_simulated_buffer_limit(Some(200));
    r.set_scene(&scene());
    r.set_light_background(true);
    assert_eq!(r.take_warning().as_deref(), Some(GPU_BUFFER_WARNING));
    assert_eq!(r.take_warning(), None, "taken once");
    let part = shot2d(&mut r);
    assert_ne!(part.data, whole.data, "some of the drawing is missing");
    // a device that takes everything: no warning
    let mut ok = renderer2d(&g, Some(240));
    assert_eq!(ok.take_warning(), None);
    assert_eq!(shot2d(&mut ok).data, whole.data);
    // a scene after a failed one is drawn again
    r.set_simulated_buffer_limit(None);
    r.set_scene(&scene());
    assert_eq!(r.take_warning(), None);
    assert_eq!(shot2d(&mut r).data, whole.data);
}

#[test]
fn a_buffer_the_device_refuses_is_an_error_value() {
    let Some(g) = gpu("try_buffer") else { return };
    let max = g.device.limits().max_buffer_size;
    // over the limit of the device: this is the size of the panic of 10 000 texts (289 411 200 bytes against 268 435 456)
    let e = crate::gpubuf::try_buffer_limited(&g.device, "too big", &[], wgpu::BufferUsages::VERTEX, None);
    assert!(e.is_ok(), "an empty buffer is padded to 4 bytes");
    let e = crate::gpubuf::try_buffer_sized(&g.device, "too big", max + 4, &[], wgpu::BufferUsages::VERTEX, None).unwrap_err();
    assert!(e.contains("over the limit"), "{e}");
    let e = crate::gpubuf::try_buffer_sized(&g.device, "huge", u64::MAX / 2, &[], wgpu::BufferUsages::VERTEX, None).unwrap_err();
    assert!(e.contains("over the limit"), "{e}");
    // a simulated smaller limit
    assert!(try_buffer_limited(&g.device, "x", &[0u8; 64], wgpu::BufferUsages::VERTEX, Some(32)).is_err());
    assert!(try_buffer_limited(&g.device, "x", &[0u8; 64], wgpu::BufferUsages::VERTEX, Some(64)).is_ok());
    // the data is in the buffer: read it back
    let b = try_buffer(&g.device, "data", &[1, 2, 3, 4, 5, 6, 7, 8], wgpu::BufferUsages::COPY_SRC).expect("buffer");
    assert_eq!(b.size(), 8);
    // the size that a device may take is what the module cuts to: half of it, at most 128 MiB
    let chunk = crate::gpubuf::chunk_bytes(&g.device);
    assert!(chunk <= max / 2 || chunk == 1 << 20);
    assert!(chunk <= crate::gpubuf::MAX_CHUNK_BYTES);
    // 289 411 200 bytes (the buffer of 10 000 texts) are cut into pieces that fit
    let corners = 289_411_200usize / 12;
    let per = crate::gpubuf::items_per_buffer(chunk, 12, 3);
    let pieces = crate::gpubuf::split_range(0, corners, per);
    assert!(pieces.len() >= 3, "{}", pieces.len());
    assert!(pieces.iter().all(|&(_, _, n)| (n as u64) * 12 <= max));
    assert_eq!(pieces.iter().map(|p| p.2).sum::<usize>(), corners);
}

// ------------------------------------------------------------------------------------------------ 3D
fn shot3d(r: &mut Renderer3d, wire: bool) -> Rgba8 {
    r.set_style(&Style { wireframe: wire, ..r.style.clone() });
    r.capture_rgba(&CaptureOptions { width: SIZE, height: SIZE, transparent_background: true, ui: true, dpr: 1.0, camera: None, msaa: None }).expect("capture")
}

fn renderer3d(g: &GpuContext, chunk: Option<u64>, limit: Option<u64>) -> Renderer3d {
    let mut r = Renderer3d::new(&g.device, wgpu::TextureFormat::Rgba8Unorm).with_queue(&g.queue);
    r.set_buffer_chunk_bytes(chunk);
    r.set_simulated_buffer_limit(limit);
    r.set_model(&[cube_mesh([None; 6])], &cube_tree(true));
    r.set_viewport(SIZE, SIZE, 1.0);
    r.set_orientation(StdView::Iso);
    r.fit_all();
    r
}

#[test]
fn a_mesh_in_many_buffers_draws_the_picture_and_the_picks_of_one() {
    let Some(g) = gpu("3d chunked buffers") else { return };
    let mut whole = renderer3d(&g, None, None);
    assert_eq!(whole.buffer_counts(), (1, 1));
    // 96 bytes: three vertices per buffer, so every triangle has a buffer of its own and the two triangles of a face (the
    // range a highlight draws) lie in two buffers
    for chunk in [96u64, 200, 1_000] {
        let mut small = renderer3d(&g, Some(chunk), None);
        let (surf, edges) = small.buffer_counts();
        if chunk <= 200 {
            assert!(surf > 1 && edges > 1, "chunk {chunk}: {surf} surface and {edges} edge buffers");
        }
        assert_eq!(small.take_warning(), None);
        for wire in [false, true] {
            assert_eq!(shot3d(&mut whole, wire).data, shot3d(&mut small, wire).data, "chunk {chunk}, wireframe {wire}");
        }
        // a hovered face (its triangles are in different buffers), a selected cube, a picked edge
        for h in [Highlight::Entity(EntityRef { node: 1, kind: EntityKind::Face, index: 4 }), Highlight::Node(2)] {
            whole.set_hover(Some(h));
            small.set_hover(Some(h));
            assert_eq!(shot3d(&mut whole, false).data, shot3d(&mut small, false).data, "chunk {chunk}, {h:?}");
        }
        whole.set_hover(None);
        small.set_hover(None);
        whole.set_selection(&[1]);
        small.set_selection(&[1]);
        assert_eq!(shot3d(&mut whole, false).data, shot3d(&mut small, false).data, "chunk {chunk}, selection");
        whole.set_selection(&[]);
        small.set_selection(&[]);
        for (x, y) in [(128.0, 128.0), (60.0, 150.0), (190.0, 100.0), (30.0, 30.0)] {
            assert_eq!(whole.pick(x, y), small.pick(x, y), "chunk {chunk}, pick {x} {y}");
        }
    }
}

#[test]
fn a_model_the_device_refuses_gives_a_warning_not_a_panic() {
    let Some(g) = gpu("3d refused buffers") else { return };
    let mut r = renderer3d(&g, None, Some(100));
    assert_eq!(r.take_warning().as_deref(), Some(GPU_BUFFER_WARNING));
    assert_eq!(r.take_warning(), None);
    // the picture is empty (nothing was taken), but it can be made
    let img = shot3d(&mut r, false);
    assert_eq!(img.pixel(SIZE / 2, SIZE / 2)[3], 0);
    assert_eq!(r.pick(128.0, 128.0), None);
    // the buffers of the frame (instances, overlays) are made under the same device: they do not fit either, and the frame
    // is drawn without them, once with a warning
    assert_eq!(r.take_warning().as_deref(), Some(crate::gpubuf::GPU_DYNAMIC_WARNING));
    assert_eq!(r.take_warning(), None);
    // the model comes back when the device takes it
    r.set_simulated_buffer_limit(None);
    r.set_model(&[cube_mesh([None; 6])], &cube_tree(true));
    assert_eq!(r.take_warning(), None);
    let img = shot3d(&mut r, false);
    assert!(img.data.chunks_exact(4).any(|p| p[3] > 0), "the model is drawn again");
}
