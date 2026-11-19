//! Drawing of one frame of the 3D view into an offscreen target, and the copy into the egui render pass.
use cad_types::Aabb;
use eframe::wgpu;
use glam::{DMat4, DVec3};

use crate::camera::{Camera, Viewport};
use crate::colors::{
    pack_rgba, to_f32, Rgb8, BG_DARK, BG_LIGHT, HIGHLIGHT_COLOR, VIEW_CUBE_BOX,
};
use crate::gpu_types::{mat4_to_array, Globals, LineInst, PointInst, ScreenVertex, LINE_PIXEL_SPACE};
use crate::overlay::OverlayLabel;
use crate::renderer3d::{FrameTarget, Renderer3d};
use crate::scalebar::scale_bar;
use crate::scene3d::{DrawRange, InstanceRaw};
use crate::text::{push_polygon, push_text, HAlign, TextFrame, VAlign};
use crate::viewcube;

/// Options of one rendered picture.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RenderOpts {
    pub transparent_background: bool,
    /// Draw view cube, scale bar, labels and overlays.
    pub ui: bool,
}

/// Lists of screen-space drawables built for one frame.
#[derive(Default)]
struct UiLists {
    /// World-space overlay lines and markers.
    lines: Vec<LineInst>,
    points: Vec<PointInst>,
    /// Pixel-space lines of the scale bar and the cube outline (drawn above the cube faces).
    ui_lines: Vec<LineInst>,
    /// Cube faces and highlights.
    polys: Vec<ScreenVertex>,
    /// All text.
    text: Vec<ScreenVertex>,
}

/// What an overlay mesh draw refers to.
enum OvSource {
    Proto(usize),
    Own(usize),
}

impl Renderer3d {
    /// Called by the paint callback before egui's main pass: renders the picture into the offscreen target.
    pub(crate) fn prepare_frame(
        &mut self,
        _device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        width: u32,
        height: u32,
        dpr: f64,
    ) {
        if width == 0 || height == 0 {
            return;
        }
        self.queue = Some(queue.clone());
        self.set_viewport(width, height, dpr);
        let stale = match &self.screen_target {
            Some(t) => t.width != width || t.height != height || t.samples != self.samples,
            None => true,
        };
        if stale {
            let t = FrameTarget::new(&self.device, width, height, self.samples);
            self.blit_bind = Some(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("blit"),
                layout: &self.layouts.blit,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&t.color_view) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.blit_sampler) },
                ],
            }));
            self.screen_target = Some(t);
        }
        let Some(target) = self.screen_target.take() else { return };
        let camera = self.camera;
        let vp = self.viewport;
        self.render_into(queue, encoder, &target, &camera, vp, dpr, &RenderOpts { transparent_background: false, ui: true });
        self.screen_target = Some(target);
    }

    /// Copies the last rendered picture into the egui pass (viewport and scissor are set by egui).
    pub(crate) fn blit(&self, pass: &mut wgpu::RenderPass<'static>) {
        if let Some(bind) = &self.blit_bind {
            pass.set_pipeline(&self.blit_pipe);
            pass.set_bind_group(0, bind, &[]);
            pass.draw(0..3, 0..1);
        }
    }

    pub(crate) fn upload_atlas(&mut self, queue: &wgpu::Queue) {
        let Some(atlas) = self.atlas.as_mut() else { return };
        if !atlas.dirty {
            return;
        }
        let s = crate::text::ATLAS_SIZE;
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.atlas_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &atlas.pixels,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(s), rows_per_image: Some(s) },
            wgpu::Extent3d { width: s, height: s, depth_or_array_layers: 1 },
        );
        atlas.dirty = false;
    }

    /// Uniforms for a camera and viewport. `crop` selects a sub-window (picking), identity for pictures.
    pub(crate) fn make_globals(&self, camera: &Camera, vp: Viewport, dpr: f64, vp_full: &DMat4, crop: &DMat4) -> Globals {
        let dir = camera.direction();
        let (right, up) = (camera.right(), camera.up_vec());
        let eye = camera.eye - self.origin;
        let (bg_top, bg_bottom) = if self.style.dark_background { BG_DARK } else { BG_LIGHT };
        let v4 = |c: Rgb8, a: f32| {
            let f = to_f32(c);
            [f[0], f[1], f[2], a]
        };
        let (clip, clip_on) = match self.section {
            Some(s) => {
                let (n, d) = s.plane();
                let shifted = d + n[0] * self.origin.x + n[1] * self.origin.y + n[2] * self.origin.z;
                ([n[0] as f32, n[1] as f32, n[2] as f32, shifted as f32], 1)
            }
            None => ([0.0, 0.0, 0.0, 0.0], 0),
        };
        Globals {
            view_proj: mat4_to_array(&(*crop * *vp_full)),
            vp_full: mat4_to_array(vp_full),
            crop: mat4_to_array(crop),
            cam_right: [right.x as f32, right.y as f32, right.z as f32, 0.0],
            cam_up: [up.x as f32, up.y as f32, up.z as f32, 0.0],
            view_dir: [dir.x as f32, dir.y as f32, dir.z as f32, 0.0],
            eye: [eye.x as f32, eye.y as f32, eye.z as f32, if camera.perspective { 1.0 } else { 0.0 }],
            clip,
            viewport: [vp.width as f32, vp.height as f32, dpr as f32, 0.0],
            cam_extent: {
                let (hx, hy) = camera.half_size(vp);
                if camera.perspective {
                    let d = camera.distance().max(1e-12);
                    [(hx / d) as f32, (hy / d) as f32, 0.0, 0.0]
                } else {
                    [hx as f32, hy as f32, 0.0, 0.0]
                }
            },
            default_color: v4(self.style.default_color, 1.0),
            edge_color: v4(self.style.edge_color, 1.0),
            bg_top: v4(bg_top, 1.0),
            bg_bottom: v4(bg_bottom, 1.0),
            flags: [clip_on, u32::from(self.style.wireframe), 0, 0],
        }
    }

    /// Full view-projection of the camera with the scene origin shifted out.
    pub(crate) fn view_proj_for(&self, camera: &Camera, vp: Viewport) -> DMat4 {
        let (c, r) = self.depth_sphere();
        camera.view_proj(vp, self.origin, c, r)
    }

    /// Pixel (physical, origin upper left) of a world point for the current camera and viewport; `None` behind the
    /// camera.
    pub fn world_to_pixel(&self, world: [f64; 3]) -> Option<[f64; 2]> {
        let vp_m = self.view_proj_for(&self.camera, self.viewport);
        let s = Camera::project_with(&vp_m, DVec3::from_array(world) - self.origin, self.viewport)?;
        Some([s.x, s.y])
    }

    /// Ray through a pixel in world coordinates: origin and unit direction.
    pub fn pixel_to_ray(&self, x: f64, y: f64) -> ([f64; 3], [f64; 3]) {
        let (o, d) = self.camera.ray(x, y, self.viewport);
        (o.to_array(), d.to_array())
    }

    /// World length of one pixel on the plane through the camera centre (`world_per_pixel`).
    pub fn world_per_pixel(&self) -> f64 {
        self.camera.world_per_pixel(self.viewport)
    }

    fn rel(&self, world: [f64; 3]) -> [f32; 3] {
        (DVec3::from_array(world) - self.origin).as_vec3().to_array()
    }

    /// Converts the user overlay lines and markers to GPU instances once (and again when the pixel ratio changes).
    fn ensure_static_overlays(&mut self, queue: &wgpu::Queue, dpr: f64) {
        if self.static_dpr == Some(dpr) {
            return;
        }
        let d = dpr as f32;
        let mut lines: Vec<LineInst> = Vec::new();
        for l in &self.overlays.lines {
            let n = l.points.len();
            if n < 2 {
                continue;
            }
            let segs = if l.closed && n > 2 { n } else { n - 1 };
            for i in 0..segs {
                lines.push(LineInst {
                    p0: self.rel(l.points[i]),
                    width: l.width * d,
                    p1: self.rel(l.points[(i + 1) % n]),
                    flags: 0,
                    color: pack_rgba(l.color, 255),
                    dash_on: if l.dashed { 8.0 * d } else { 0.0 },
                    dash_off: if l.dashed { 5.0 * d } else { 0.0 },
                    pad: 0.0,
                });
            }
        }
        let points: Vec<PointInst> = self
            .overlays
            .points
            .iter()
            .map(|p| PointInst {
                pos: self.rel(p.pos),
                size: p.size * d,
                color: pack_rgba(p.color, 255),
                flags: 0,
                pad: [0; 2],
            })
            .collect();
        self.static_lines.upload(&self.device, queue, bytemuck::cast_slice(&lines));
        self.static_points.upload(&self.device, queue, bytemuck::cast_slice(&points));
        self.static_dpr = Some(dpr);
    }

    fn build_lists(&mut self, camera: &Camera, vp: Viewport, dpr: f64, vp_m: &DMat4, opts: &RenderOpts) -> UiLists {
        let mut out = UiLists::default();
        if !opts.ui {
            return out;
        }
        let d = dpr as f32;
        // ---- world overlays and entity highlights
        let mut labels: Vec<OverlayLabel> = Vec::new();
        labels.extend(self.overlays.labels.iter().cloned());
        for o in [&self.hl_overlay] {
            for l in &o.lines {
                let n = l.points.len();
                if n < 2 {
                    continue;
                }
                let segs = if l.closed && n > 2 { n } else { n - 1 };
                for i in 0..segs {
                    let (a, b) = (l.points[i], l.points[(i + 1) % n]);
                    out.lines.push(LineInst {
                        p0: self.rel(a),
                        width: l.width * d,
                        p1: self.rel(b),
                        flags: 0,
                        color: pack_rgba(l.color, 255),
                        dash_on: if l.dashed { 8.0 * d } else { 0.0 },
                        dash_off: if l.dashed { 5.0 * d } else { 0.0 },
                        pad: 0.0,
                    });
                }
            }
            for p in &o.points {
                out.points.push(PointInst {
                    pos: self.rel(p.pos),
                    size: p.size * d,
                    color: pack_rgba(p.color, 255),
                    flags: 0,
                    pad: [0; 2],
                });
            }
            labels.extend(o.labels.iter().cloned());
        }
        if let Some(atlas) = self.atlas.as_mut() {
            for l in &labels {
                let p = DVec3::from_array(l.pos) - self.origin;
                let Some(s) = Camera::project_with(vp_m, p, vp) else { continue };
                push_text(
                    &mut out.text,
                    atlas,
                    &l.text,
                    [s.x as f32, s.y as f32],
                    l.height * d,
                    pack_rgba(l.color, 255),
                    l.h,
                    l.v,
                    TextFrame::default(),
                );
            }
        }

        // ---- scale bar
        if self.style.scale_bar {
            if let (Some(bar), Some(atlas)) = (scale_bar(camera, vp, dpr, self.style.dark_background), self.atlas.as_mut()) {
                let (ox, oy) = (bar.offset_px[0] as f32, vp.height as f32 - bar.offset_px[1] as f32);
                let col = pack_rgba(bar.color, 255);
                for (a, b) in &bar.segments {
                    out.ui_lines.push(LineInst {
                        p0: [ox + a[0] as f32, oy - a[1] as f32, 0.0],
                        width: bar.line_width_px as f32,
                        p1: [ox + b[0] as f32, oy - b[1] as f32, 0.0],
                        flags: LINE_PIXEL_SPACE,
                        color: col,
                        dash_on: 0.0,
                        dash_off: 0.0,
                        pad: 0.0,
                    });
                }
                push_text(
                    &mut out.text,
                    atlas,
                    &bar.label,
                    [ox + bar.label_anchor[0] as f32, oy - bar.label_anchor[1] as f32],
                    bar.label_height_px as f32,
                    col,
                    HAlign::Center,
                    VAlign::Bottom,
                    TextFrame::default(),
                );
            }
        }

        // ---- view cube
        if self.style.view_cube {
            let layout = viewcube::CubeLayout::new(vp, dpr);
            let faces = viewcube::faces(camera, &layout);
            if let Some(atlas) = self.atlas.as_mut() {
                for f in &faces {
                    let shade = f.shade;
                    let base = to_f32(VIEW_CUBE_BOX);
                    let c = pack_rgba(
                        [(base[0] * shade * 255.0) as u8, (base[1] * shade * 255.0) as u8, (base[2] * shade * 255.0) as u8],
                        255,
                    );
                    let pts: Vec<[f32; 2]> = f.corners.iter().map(|p| [p[0] as f32, p[1] as f32]).collect();
                    push_polygon(&mut out.polys, atlas, &pts, c);
                    for i in 0..4 {
                        let (a, b) = (f.corners[i], f.corners[(i + 1) % 4]);
                        out.ui_lines.push(LineInst {
                            p0: [a[0] as f32, a[1] as f32, 0.0],
                            width: 1.0,
                            p1: [b[0] as f32, b[1] as f32, 0.0],
                            flags: LINE_PIXEL_SPACE,
                            color: pack_rgba([90, 96, 110], 255),
                            dash_on: 0.0,
                            dash_off: 0.0,
                            pad: 0.0,
                        });
                    }
                }
                if let Some(h) = self.cube_hover {
                    for q in viewcube::region_quads(camera, &layout, h) {
                        let pts: Vec<[f32; 2]> = q.iter().map(|p| [p[0] as f32, p[1] as f32]).collect();
                        push_polygon(&mut out.polys, atlas, &pts, pack_rgba(HIGHLIGHT_COLOR, 170));
                    }
                }
                for f in &faces {
                    push_text(
                        &mut out.text,
                        atlas,
                        f.label,
                        [f.label_center[0] as f32, f.label_center[1] as f32],
                        (viewcube::FONT_PX * dpr) as f32,
                        pack_rgba([40, 44, 52], 255),
                        HAlign::Center,
                        VAlign::Center,
                        TextFrame {
                            right: [f.label_right[0] as f32, f.label_right[1] as f32],
                            up: [f.label_up[0] as f32, f.label_up[1] as f32],
                        },
                    );
                }
            }
        }
        out
    }

    /// Renders the scene for `camera` into `target` (resolved into its single-sample colour texture).
    pub(crate) fn render_into(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &FrameTarget,
        camera: &Camera,
        vp: Viewport,
        dpr: f64,
        opts: &RenderOpts,
    ) {
        self.flush_instances(queue);
        if self.hl_dirty {
            self.rebuild_entity_highlights();
        }
        let vp_m = self.view_proj_for(camera, vp);
        if opts.ui {
            self.ensure_static_overlays(queue, dpr);
        }
        let lists = self.build_lists(camera, vp, dpr, &vp_m, opts);
        self.upload_atlas(queue);
        let globals = self.make_globals(camera, vp, dpr, &vp_m, &DMat4::IDENTITY);
        queue.write_buffer(&self.globals_buf, 0, bytemuck::bytes_of(&globals));

        // dynamic buffers: world lines then ui lines share one buffer, polygons then text share another
        let mut all_lines = lists.lines.clone();
        let n_world_lines = all_lines.len() as u32;
        all_lines.extend_from_slice(&lists.ui_lines);
        let n_lines = all_lines.len() as u32;
        self.line_buf.upload(&self.device, queue, bytemuck::cast_slice(&all_lines));
        self.point_buf.upload(&self.device, queue, bytemuck::cast_slice(&lists.points));
        let n_poly = lists.polys.len() as u32;
        let mut screen = lists.polys;
        screen.extend_from_slice(&lists.text);
        let n_screen = screen.len() as u32;
        self.screen_buf.upload(&self.device, queue, bytemuck::cast_slice(&screen));

        // overlay meshes and highlighted faces
        let mut ov_raws: Vec<InstanceRaw> = Vec::new();
        let mut ov_draws: Vec<(OvSource, u32, u32)> = Vec::new();
        if opts.ui {
            for (i, m) in self.overlay_meshes.iter().enumerate() {
                ov_raws.push(m.raw);
                ov_draws.push((OvSource::Own(i), 0, m.gpu.index_count));
            }
            for h in &self.hl_faces {
                ov_raws.push(h.raw);
                ov_draws.push((OvSource::Proto(h.proto), h.first, h.count));
            }
        }
        self.ov_inst_buf.upload(&self.device, queue, bytemuck::cast_slice(&ov_raws));

        let clear = if opts.transparent_background { wgpu::Color::TRANSPARENT } else { wgpu::Color::BLACK };
        let (view, resolve) = match &target.msaa_view {
            Some(m) => (m, Some(&target.color_view)),
            None => (&target.color_view, None),
        };
        // ---- pass A: background, surfaces, edges, transparent surfaces
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: resolve,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(clear), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &target.depth_view,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                    stencil_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0), store: wgpu::StoreOp::Store }),
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.globals_bind, &[]);
            if !opts.transparent_background {
                pass.set_pipeline(&self.pipelines.bg);
                pass.draw(0..3, 0..1);
            }
            let wire = self.style.wireframe;
            if !wire {
                if self.section.is_some() {
                    // front faces, then the cap (back faces seen through the cut, marked in the stencil), then the
                    // marked pixels are moved to the depth of the plane
                    pass.set_pipeline(&self.pipelines.cap_front);
                    self.draw_meshes(&mut pass, &self.layout.opaque);
                    pass.set_pipeline(&self.pipelines.cap_mark);
                    pass.set_stencil_reference(1);
                    self.draw_meshes(&mut pass, &self.layout.opaque);
                    pass.set_pipeline(&self.pipelines.cap_depth);
                    pass.set_stencil_reference(1);
                    pass.draw(0..3, 0..1);
                } else {
                    pass.set_pipeline(&self.pipelines.mesh_opaque);
                    self.draw_meshes(&mut pass, &self.layout.opaque);
                }
            }
            if self.style.edges || wire {
                pass.set_pipeline(&self.pipelines.edges);
                self.draw_edges(&mut pass, &self.layout.opaque);
                self.draw_edges(&mut pass, &self.layout.transparent);
            }
            if !wire {
                pass.set_pipeline(&self.pipelines.mesh_transparent);
                self.draw_meshes(&mut pass, &self.layout.transparent);
            }
        }
        if !opts.ui {
            return;
        }
        // ---- pass B: overlays on a cleared depth buffer
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("overlays"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: resolve,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &target.depth_view,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                    stencil_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0), store: wgpu::StoreOp::Store }),
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.globals_bind, &[]);
            if !ov_draws.is_empty() {
                if let Some(inst) = &self.ov_inst_buf.buf {
                    pass.set_pipeline(&self.pipelines.mesh_overlay);
                    pass.set_vertex_buffer(1, inst.slice(..));
                    for (k, (src, first, count)) in ov_draws.iter().enumerate() {
                        let gpu = match src {
                            OvSource::Own(i) => &self.overlay_meshes[*i].gpu,
                            OvSource::Proto(p) => &self.protos[*p],
                        };
                        // the indices `first..first + count` of the prototype lie in one or more pieces of its buffers
                        for part in &gpu.parts {
                            part.draw_range(&mut pass, *first, *count, k as u32..k as u32 + 1);
                        }
                    }
                }
            }
            if let (Some(sb), true) = (&self.static_lines.buf, self.static_lines.len > 0) {
                pass.set_pipeline(&self.pipelines.lines);
                pass.set_vertex_buffer(0, sb.slice(..));
                pass.draw(0..6, 0..(self.static_lines.len / 48) as u32);
            }
            if let Some(lb) = &self.line_buf.buf {
                if n_world_lines > 0 {
                    pass.set_pipeline(&self.pipelines.lines);
                    pass.set_vertex_buffer(0, lb.slice(..));
                    pass.draw(0..6, 0..n_world_lines);
                }
            }
            if let (Some(sb), true) = (&self.static_points.buf, self.static_points.len > 0) {
                pass.set_pipeline(&self.pipelines.points);
                pass.set_vertex_buffer(0, sb.slice(..));
                pass.draw(0..6, 0..(self.static_points.len / 32) as u32);
            }
            if let (Some(pb), true) = (&self.point_buf.buf, self.point_buf.len > 0) {
                pass.set_pipeline(&self.pipelines.points);
                pass.set_vertex_buffer(0, pb.slice(..));
                pass.draw(0..6, 0..(self.point_buf.len / 32) as u32);
            }
            if let Some(sb) = &self.screen_buf.buf {
                if n_poly > 0 {
                    pass.set_pipeline(&self.pipelines.screen);
                    pass.set_bind_group(1, &self.atlas_bind, &[]);
                    pass.set_vertex_buffer(0, sb.slice(..));
                    pass.draw(0..n_poly, 0..1);
                }
            }
            if let Some(lb) = &self.line_buf.buf {
                if n_lines > n_world_lines {
                    pass.set_pipeline(&self.pipelines.lines);
                    pass.set_vertex_buffer(0, lb.slice(..));
                    pass.draw(0..6, n_world_lines..n_lines);
                }
            }
            if let Some(sb) = &self.screen_buf.buf {
                if n_screen > n_poly {
                    pass.set_pipeline(&self.pipelines.screen);
                    pass.set_bind_group(1, &self.atlas_bind, &[]);
                    pass.set_vertex_buffer(0, sb.slice(..));
                    pass.draw(n_poly..n_screen, 0..1);
                }
            }
        }
    }

    fn draw_meshes(&self, pass: &mut wgpu::RenderPass<'_>, ranges: &[DrawRange]) {
        let Some(inst) = &self.inst_buf.buf else { return };
        pass.set_vertex_buffer(1, inst.slice(..));
        for r in ranges {
            let Some(p) = self.protos.get(r.proto as usize) else { continue };
            for part in &p.parts {
                part.draw_range(pass, 0, p.index_count, r.first..r.first + r.count);
            }
        }
    }

    fn draw_edges(&self, pass: &mut wgpu::RenderPass<'_>, ranges: &[DrawRange]) {
        let Some(inst) = &self.inst_buf.buf else { return };
        pass.set_vertex_buffer(1, inst.slice(..));
        for r in ranges {
            let Some(p) = self.protos.get(r.proto as usize) else { continue };
            for part in &p.eparts {
                part.draw_range(pass, 0, p.edge_index_count, r.first..r.first + r.count);
            }
        }
    }

    /// Draws the instances of every range with the vertex-pick pipeline (storage buffer per prototype).
    pub(crate) fn draw_vertex_pick(&self, pass: &mut wgpu::RenderPass<'_>, ranges: &[DrawRange]) {
        let Some(inst) = &self.inst_buf.buf else { return };
        pass.set_vertex_buffer(0, inst.slice(..));
        for r in ranges {
            let Some(p) = self.protos.get(r.proto as usize) else { continue };
            let Some(bind) = &p.pts_bind else { continue };
            pass.set_bind_group(1, bind, &[]);
            pass.draw(0..6 * p.pts_count, r.first..r.first + r.count);
        }
    }

    pub(crate) fn draw_meshes_pick(&self, pass: &mut wgpu::RenderPass<'_>) {
        self.draw_meshes(pass, &self.layout.opaque);
        self.draw_meshes(pass, &self.layout.transparent);
    }

    pub(crate) fn draw_edges_pick(&self, pass: &mut wgpu::RenderPass<'_>) {
        self.draw_edges(pass, &self.layout.opaque);
        self.draw_edges(pass, &self.layout.transparent);
    }

    /// Bounds of the whole content for tests and tools.
    pub fn content_bounds(&self) -> Option<Aabb> {
        self.all_bounds
    }
}
