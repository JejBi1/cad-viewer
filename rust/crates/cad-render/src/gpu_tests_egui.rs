//! The paint callbacks of both renderers inside a real `egui_wgpu::Renderer` pass (headless).
use std::sync::{Arc, Mutex};

use eframe::{egui, egui_wgpu, wgpu};

use crate::capture::Rgba8;
use crate::gpu::GpuContext;
use crate::gpu_tests::{cube_mesh, cube_tree, gpu};
use crate::pickpass::read_buffer;
use crate::renderer2d::Renderer2d;
use crate::renderer3d::Renderer3d;
use crate::StdView;

const W_PTS: f32 = 300.0;
const H_PTS: f32 = 200.0;

/// Runs one egui frame that adds the given paint callbacks and renders it with egui's wgpu renderer.
fn egui_frame(g: &GpuContext, ppp: f32, callbacks: impl Fn(&egui::Context) -> Vec<egui::PaintCallback>) -> Rgba8 {
    egui_frame_in(g, wgpu::TextureFormat::Rgba8Unorm, ppp, callbacks)
}

/// Same with a chosen colour format of the egui render target.
fn egui_frame_in(
    g: &GpuContext,
    format: wgpu::TextureFormat,
    ppp: f32,
    callbacks: impl Fn(&egui::Context) -> Vec<egui::PaintCallback>,
) -> Rgba8 {
    egui_frame_with(g, format, 1, None, ppp, callbacks)
}

/// Same with a multisampled egui pass and / or a depth attachment (what `eframe` does with `multisampling` and
/// `depth_buffer` set).
fn egui_frame_with(
    g: &GpuContext,
    format: wgpu::TextureFormat,
    samples: u32,
    depth: Option<wgpu::TextureFormat>,
    ppp: f32,
    callbacks: impl Fn(&egui::Context) -> Vec<egui::PaintCallback>,
) -> Rgba8 {
    let ctx = egui::Context::default();
    ctx.set_pixels_per_point(ppp);
    let raw = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(W_PTS, H_PTS))),
        ..Default::default()
    };
    let cbs = callbacks(&ctx);
    let mut out = ctx.run_ui(raw, |ui| {
        for cb in &cbs {
            ui.painter().add(cb.clone());
        }
    });
    let jobs = ctx.tessellate(out.shapes, out.pixels_per_point);
    let size = [(W_PTS * ppp) as u32, (H_PTS * ppp) as u32];
    let sd = egui_wgpu::ScreenDescriptor { size_in_pixels: size, pixels_per_point: out.pixels_per_point };
    let mut er = egui_wgpu::Renderer::new(
        &g.device,
        format,
        egui_wgpu::RendererOptions { msaa_samples: samples, depth_stencil_format: depth, ..Default::default() },
    );
    for (id, deltas) in &out.textures_delta.set {
        for delta in deltas {
            er.update_texture(&g.device, &g.queue, *id, delta);
        }
    }
    out.textures_delta.clear();
    let extent = wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 };
    let target = g.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("egui target"),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let attachment = |label: &str, format, count| {
        g.device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: extent,
                mip_level_count: 1,
                sample_count: count,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor::default())
    };
    let msaa_view = (samples > 1).then(|| attachment("egui msaa", format, samples));
    let depth_view = depth.map(|d| attachment("egui depth", d, samples.max(1)));
    let mut enc = g.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("egui") });
    let extra = er.update_buffers(&g.device, &g.queue, &mut enc, &jobs, &sd);
    {
        let mut pass = enc
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("egui pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: msaa_view.as_ref().unwrap_or(&view),
                    depth_slice: None,
                    resolve_target: msaa_view.as_ref().map(|_| &view),
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: depth_view.as_ref().map(|v| wgpu::RenderPassDepthStencilAttachment {
                    view: v,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                    stencil_ops: if depth.is_some_and(|d| d.has_stencil_aspect()) {
                        Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0), store: wgpu::StoreOp::Discard })
                    } else {
                        None
                    },
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            })
            .forget_lifetime();
        er.render(&mut pass, &jobs, &sd);
    }
    let bpr = (size[0] * 4).div_ceil(256) * 256;
    let buffer = g.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("egui readback"),
        size: u64::from(bpr) * u64::from(size[1]),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo { texture: &target, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bpr), rows_per_image: Some(size[1]) },
        },
        extent,
    );
    g.queue.submit(extra.into_iter().chain(Some(enc.finish())));
    let raw_data = read_buffer(&g.device, &buffer).expect("read back");
    let mut data = Vec::new();
    for y in 0..size[1] {
        let o = (y * bpr) as usize;
        data.extend_from_slice(&raw_data[o..o + (size[0] * 4) as usize]);
    }
    Rgba8 { width: size[0], height: size[1], data }
}

fn shared_3d(g: &GpuContext) -> Arc<Mutex<Renderer3d>> {
    // the renderer is created without a queue: the callback attaches it, exactly as in the application
    let mut r = Renderer3d::new(&g.device, wgpu::TextureFormat::Rgba8Unorm);
    r.set_model(&[cube_mesh([None; 6])], &cube_tree(false));
    r.camera.set_view(StdView::Front);
    r.shared()
}

#[test]
fn renderer3d_inside_an_egui_pass() {
    let Some(g) = gpu("renderer3d_inside_an_egui_pass") else { return };
    let shared = shared_3d(&g);
    let rect = egui::Rect::from_min_size(egui::pos2(50.0, 20.0), egui::vec2(200.0, 150.0));
    for ppp in [1.0f32, 2.0] {
        // first frame: the viewport is only known inside the callback, so fit after it was set
        let s2 = shared.clone();
        let img = egui_frame(&g, ppp, |_| vec![Renderer3d::paint_callback(&s2, rect)]);
        {
            let mut r = shared.lock().expect("lock");
            assert_eq!(r.viewport().width, f64::from(200.0 * ppp));
            assert_eq!(r.viewport().height, f64::from(150.0 * ppp));
            assert!((r.dpr() - f64::from(ppp)).abs() < 1e-9);
            r.fit_all();
        }
        let s3 = shared.clone();
        let img2 = egui_frame(&g, ppp, |_| vec![Renderer3d::paint_callback(&s3, rect)]);
        if ppp == 1.0 {
            assert_ne!(img, img2, "fit_all changed the picture");
        }
        let px = |x: f32, y: f32| img2.pixel((x * ppp) as u32, (y * ppp) as u32);
        // outside the rectangle: egui's clear colour
        assert_eq!(px(10.0, 10.0), [0, 0, 0, 255], "outside the callback rectangle only the clear colour of the egui pass");
        // upper left corner of the callback rectangle: gradient background (top colour)
        let c = px(52.0, 22.0);
        assert!((i32::from(c[0]) - 230).abs() <= 4 && (i32::from(c[1]) - 235).abs() <= 4, "{c:?}");
        // the front face of the cube in the part colour (left of the view cube, which covers the middle of this small view)
        let m = px(100.0, 120.0);
        assert_eq!(m[3], 255);
        assert!((i32::from(m[0]) - 168).abs() <= 4 && (i32::from(m[2]) - 176).abs() <= 4, "{m:?}");
        // below the rectangle: nothing
        assert_eq!(px(150.0, 185.0), [0, 0, 0, 255]);
        // picking uses the viewport set by the callback and the queue attached by it
        let mut r = shared.lock().expect("lock");
        let hit = r.pick(100.0 * ppp, 75.0 * ppp).expect("face at the middle of the view");
        assert_eq!((hit.node, hit.index), (1, 3));
    }
    // a different size recreates the target
    let rect2 = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(120.0, 90.0));
    let s4 = shared.clone();
    let img = egui_frame(&g, 1.0, |_| vec![Renderer3d::paint_callback(&s4, rect2)]);
    assert_eq!(img.pixel(60, 45)[3], 255);
    assert_eq!(shared.lock().expect("lock").viewport().width, 120.0);
}

#[test]
fn renderer2d_inside_an_egui_pass() {
    let Some(g) = gpu("renderer2d_inside_an_egui_pass") else { return };
    let mut r2 = Renderer2d::new(&g.device, wgpu::TextureFormat::Rgba8Unorm);
    let scene = cad_types::Scene2d {
        layers: vec![cad_types::Layer { id: 0, name: "0".into(), entities: 1, visible: true, off_in_file: false }],
        batches: vec![cad_types::DrawBatch {
            layer: 0,
            color: [255, 0, 0, 255],
            width: 0.0,
            lines: vec![],
            fills: vec![[0.0, 0.0], [100.0, 0.0], [100.0, 100.0], [0.0, 0.0], [100.0, 100.0], [0.0, 100.0]],
        }],
        bounds: Some(([0.0, 0.0], [100.0, 100.0])),
        layout: "Model".into(),
        layouts: vec![],
        ..Default::default()
    };
    r2.set_scene(&scene);
    r2.set_light_background(true);
    let shared = r2.shared();
    let rect = egui::Rect::from_min_size(egui::pos2(50.0, 20.0), egui::vec2(150.0, 150.0));
    let s = shared.clone();
    let _ = egui_frame(&g, 1.0, |_| vec![Renderer2d::paint_callback(&s, rect)]);
    shared.lock().expect("lock").fit();
    let s = shared.clone();
    let img = egui_frame(&g, 1.0, |_| vec![Renderer2d::paint_callback(&s, rect)]);
    assert_eq!(img.pixel(10, 10), [0, 0, 0, 255], "outside");
    assert_eq!(img.pixel(52, 22), [255, 255, 255, 255], "white background inside");
    assert_eq!(img.pixel(125, 95), [255, 0, 0, 255], "the filled square in the middle");
}

#[test]
fn renderer3d_in_a_srgb_egui_pass_matches_the_gamma_pass() {
    let Some(g) = gpu("renderer3d_in_a_srgb_egui_pass") else { return };
    let rect = egui::Rect::from_min_size(egui::pos2(50.0, 20.0), egui::vec2(200.0, 150.0));
    let draw = |format: wgpu::TextureFormat| {
        let mut r = Renderer3d::new(&g.device, format);
        r.set_model(&[cube_mesh([None; 6])], &cube_tree(false));
        r.camera.set_view(StdView::Front);
        let shared = r.shared();
        let s = shared.clone();
        let _ = egui_frame_in(&g, format, 1.0, |_| vec![Renderer3d::paint_callback(&s, rect)]);
        shared.lock().expect("lock").fit_all();
        let s = shared.clone();
        egui_frame_in(&g, format, 1.0, |_| vec![Renderer3d::paint_callback(&s, rect)])
    };
    let gamma = draw(wgpu::TextureFormat::Rgba8Unorm);
    let srgb = draw(wgpu::TextureFormat::Rgba8UnormSrgb);
    // the bytes of the sRGB target are sRGB-encoded by the hardware: same numbers as the gamma path
    for (x, y) in [(52, 22), (100, 120), (240, 160), (60, 150)] {
        let (a, b) = (gamma.pixel(x, y), srgb.pixel(x, y));
        assert!((0..3).all(|i| (i32::from(a[i]) - i32::from(b[i])).abs() <= 3), "({x},{y}): {a:?} vs {b:?}");
    }
}

#[test]
fn renderers_inside_a_multisampled_egui_pass_with_depth() {
    let Some(g) = gpu("renderers_inside_a_multisampled_egui_pass_with_depth") else { return };
    let rect = egui::Rect::from_min_size(egui::pos2(50.0, 20.0), egui::vec2(200.0, 150.0));
    let info = crate::TargetInfo {
        format: wgpu::TextureFormat::Rgba8Unorm,
        samples: 4,
        depth: Some(wgpu::TextureFormat::Depth32Float),
    };
    let mut r = Renderer3d::with_target(&g.device, info);
    r.set_model(&[cube_mesh([None; 6])], &cube_tree(false));
    r.camera.set_view(StdView::Front);
    assert_eq!(r.target_info(), info);
    let shared = r.shared();
    let draw = |shared: &Arc<Mutex<Renderer3d>>| {
        let s = shared.clone();
        egui_frame_with(&g, info.format, 4, info.depth, 1.0, |_| vec![Renderer3d::paint_callback(&s, rect)])
    };
    let _ = draw(&shared);
    shared.lock().expect("lock").fit_all();
    let img = draw(&shared);
    let m = img.pixel(100, 120);
    assert!((i32::from(m[0]) - 168).abs() <= 4, "front face: {m:?}");
    assert_eq!(img.pixel(10, 10), [0, 0, 0, 255]);

    // the 2D renderer in the same kind of pass
    let mut r2 = Renderer2d::with_target(&g.device, info);
    r2.set_scene(&cad_types::Scene2d {
        layers: vec![],
        batches: vec![cad_types::DrawBatch {
            layer: 0,
            color: [0, 0, 255, 255],
            width: 0.0,
            lines: vec![],
            fills: vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]],
        }],
        bounds: Some(([0.0, 0.0], [10.0, 10.0])),
        layout: String::new(),
        layouts: vec![],
        ..Default::default()
    });
    let shared2 = r2.shared();
    let s = shared2.clone();
    let _ = egui_frame_with(&g, info.format, 4, info.depth, 1.0, |_| vec![Renderer2d::paint_callback(&s, rect)]);
    shared2.lock().expect("lock").fit();
    let s = shared2.clone();
    let img = egui_frame_with(&g, info.format, 4, info.depth, 1.0, |_| vec![Renderer2d::paint_callback(&s, rect)]);
    // lower right triangle of the square is blue
    let c = img.pixel(180, 140);
    assert_eq!(c[2], 255, "{c:?}");
}
