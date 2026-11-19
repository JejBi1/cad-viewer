//! GPU id-buffer picking: the scene is rendered into tiny `Rg32Uint` targets that cover only the window around the
//! cursor, then the window is read back and resolved by [`crate::pick::resolve`].
use std::sync::mpsc;

use cad_types::{EntityKind, EntityRef};
use eframe::wgpu;
use glam::{DMat4, DVec3, DVec4};

use crate::camera::{Camera, Viewport};
use crate::gpu::RenderError;
use crate::interaction::pick_tolerance;
use crate::pick::{resolve, window_size, PickFilter, PickWindow};
use crate::pipelines::{DEPTH_FORMAT, PICK_FORMAT};
use crate::renderer3d::{PickHit, Renderer3d};

/// Matrix that maps the window of `2 * radius + 1` pixels around pixel `(x, y)` onto the whole clip space, so that
/// a tiny target sees exactly those pixels (the equivalent of `gluPickMatrix`).
pub fn pick_crop(x: i64, y: i64, radius: u32, vp: Viewport) -> DMat4 {
    let w = f64::from(window_size(radius));
    let left = (x - i64::from(radius)) as f64;
    let top = (y - i64::from(radius)) as f64;
    let cx = (left + w / 2.0) / vp.width * 2.0 - 1.0;
    let cy = 1.0 - (top + w / 2.0) / vp.height * 2.0;
    let sx = vp.width / w;
    let sy = vp.height / w;
    DMat4::from_cols(
        DVec4::new(sx, 0.0, 0.0, 0.0),
        DVec4::new(0.0, sy, 0.0, 0.0),
        DVec4::new(0.0, 0.0, 1.0, 0.0),
        DVec4::new(-sx * cx, -sy * cy, 0.0, 1.0),
    )
}

fn align256(n: u32) -> u32 {
    n.div_ceil(256) * 256
}

impl Renderer3d {
    /// Entity under the cursor (`x`, `y` in physical pixels from the upper left of the view): vertices, edges and
    /// faces with the 7 px snap tolerance while measuring, otherwise faces within 3 px (edges in wireframe).
    /// `None` when nothing is hit or no queue is attached yet.
    pub fn pick(&mut self, x: f32, y: f32) -> Option<EntityRef> {
        self.pick_hit(x, y).map(|h| h.entity)
    }

    /// Like [`Renderer3d::pick`] with the world position of the hit.
    pub fn pick_hit(&mut self, x: f32, y: f32) -> Option<PickHit> {
        let filter = self.pick_filter();
        let radius = pick_tolerance(self.measuring, self.dpr);
        self.pick_with(x, y, radius, filter).ok().flatten()
    }

    /// Model point under the cursor (rotation pivot, `_pick_pivot`), else `None`.
    pub fn pivot_at(&mut self, x: f32, y: f32) -> Option<[f64; 3]> {
        let radius = pick_tolerance(false, self.dpr);
        let filter = if self.style.wireframe { PickFilter::EDGES } else { PickFilter::FACES };
        self.pick_with(x, y, radius, filter).ok().flatten().map(|h| h.point)
    }

    /// Full control: window radius in pixels and what may be hit.
    pub fn pick_with(&mut self, x: f32, y: f32, radius: u32, filter: PickFilter) -> Result<Option<PickHit>, RenderError> {
        let queue = self.queue_or_err()?;
        let vp = self.viewport;
        if !(x.is_finite() && y.is_finite()) || x < -(radius as f32) || y < -(radius as f32) {
            return Ok(None);
        }
        if f64::from(x) >= vp.width + f64::from(radius) || f64::from(y) >= vp.height + f64::from(radius) {
            return Ok(None);
        }
        if self.instances.is_empty() {
            return Ok(None);
        }
        self.flush_instances(&queue);
        let (xi, yi) = (x.floor() as i64, y.floor() as i64);
        let size = window_size(radius);
        let camera = self.camera;
        let vp_m = self.view_proj_for(&camera, vp);
        let crop = pick_crop(xi, yi, radius, vp);
        let globals = self.make_globals(&camera, vp, self.dpr, &vp_m, &crop);
        queue.write_buffer(&self.pick_globals_buf, 0, bytemuck::bytes_of(&globals));

        let device = self.device.clone();
        let extent = wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 };
        let mk = |label: &str, format| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: extent,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            })
        };
        let tex_faces = mk("pick faces", PICK_FORMAT);
        let tex_edges = mk("pick edges", PICK_FORMAT);
        let tex_verts = mk("pick vertices", PICK_FORMAT);
        let tex_depth = mk("pick depth", DEPTH_FORMAT);
        let view = |t: &wgpu::Texture| t.create_view(&wgpu::TextureViewDescriptor::default());
        let (v_faces, v_edges, v_verts, v_depth) = (view(&tex_faces), view(&tex_edges), view(&tex_verts), view(&tex_depth));

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("pick") });
        let mut depth_cleared = false;
        let mut run = |encoder: &mut wgpu::CommandEncoder, color: &wgpu::TextureView, draw: &dyn Fn(&mut wgpu::RenderPass<'_>)| {
            let load = if depth_cleared { wgpu::LoadOp::Load } else { wgpu::LoadOp::Clear(1.0) };
            depth_cleared = true;
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("pick"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: color,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &v_depth,
                    depth_ops: Some(wgpu::Operations { load, store: wgpu::StoreOp::Store }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.pick_globals_bind, &[]);
            draw(&mut pass);
        };
        if filter.faces && !self.style.wireframe {
            run(&mut encoder, &v_faces, &|pass| {
                pass.set_pipeline(&self.pick_pipes.mesh);
                self.draw_meshes_pick(pass);
            });
        }
        if filter.edges {
            run(&mut encoder, &v_edges, &|pass| {
                pass.set_pipeline(&self.pick_pipes.edges);
                self.draw_edges_pick(pass);
            });
        }
        if filter.vertices {
            run(&mut encoder, &v_verts, &|pass| {
                pass.set_pipeline(&self.pick_pipes.vertices);
                self.draw_vertex_pick(pass, &self.layout.opaque);
                self.draw_vertex_pick(pass, &self.layout.transparent);
            });
        }

        // read everything back through one buffer
        let id_bpr = align256(size * 8);
        let depth_bpr = align256(size * 4);
        let id_len = u64::from(id_bpr) * u64::from(size);
        let depth_len = u64::from(depth_bpr) * u64::from(size);
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pick readback"),
            size: 3 * id_len + depth_len,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        for (i, (tex, aspect, bpr)) in [
            (&tex_faces, wgpu::TextureAspect::All, id_bpr),
            (&tex_edges, wgpu::TextureAspect::All, id_bpr),
            (&tex_verts, wgpu::TextureAspect::All, id_bpr),
            (&tex_depth, wgpu::TextureAspect::DepthOnly, depth_bpr),
        ]
        .into_iter()
        .enumerate()
        {
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo { texture: tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect },
                wgpu::TexelCopyBufferInfo {
                    buffer: &readback,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: i as u64 * id_len,
                        bytes_per_row: Some(bpr),
                        rows_per_image: Some(size),
                    },
                },
                extent,
            );
        }
        queue.submit(Some(encoder.finish()));
        let data = read_buffer(&device, &readback)?;

        let cells = |base: u64| -> Vec<[u32; 2]> {
            let mut out = Vec::with_capacity((size * size) as usize);
            for j in 0..size {
                for i in 0..size {
                    let o = (base + u64::from(j) * u64::from(id_bpr) + u64::from(i) * 8) as usize;
                    let a = u32::from_le_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]);
                    let b = u32::from_le_bytes([data[o + 4], data[o + 5], data[o + 6], data[o + 7]]);
                    out.push([a, b]);
                }
            }
            out
        };
        let mut depth = Vec::with_capacity((size * size) as usize);
        for j in 0..size {
            for i in 0..size {
                let o = (3 * id_len + u64::from(j) * u64::from(depth_bpr) + u64::from(i) * 4) as usize;
                depth.push(f32::from_le_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]));
            }
        }
        let window = PickWindow { size, faces: cells(0), edges: cells(id_len), vertices: cells(2 * id_len), depth };
        let Some(raw) = resolve(&window, filter, radius) else { return Ok(None) };
        let Some(inst) = self.instances.get(raw.instance as usize) else { return Ok(None) };
        let entity = EntityRef { node: inst.node, kind: raw.kind, index: raw.index };
        let point = if raw.kind == EntityKind::Vertex {
            self.protos
                .get(inst.proto as usize)
                .and_then(|p| p.data.points.get(raw.index as usize))
                .map(|p| inst.model.transform_point3(DVec3::new(f64::from(p[0]), f64::from(p[1]), f64::from(p[2]))).to_array())
        } else {
            None
        };
        let point = point.or_else(|| {
            let px = f64::from(xi as i32 + raw.dx) + 0.5;
            let py = f64::from(yi as i32 + raw.dy) + 0.5;
            Camera::unproject_with(&vp_m, px, py, f64::from(raw.depth), vp).map(|p| (p + self.origin).to_array())
        });
        Ok(point.map(|point| PickHit { entity, point, pixel_distance: raw.distance() }))
    }
}

/// Maps a `MAP_READ` buffer, waits for the GPU and returns a copy of its bytes.
pub(crate) fn read_buffer(device: &wgpu::Device, buffer: &wgpu::Buffer) -> Result<Vec<u8>, RenderError> {
    let (tx, rx) = mpsc::channel();
    buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|e| RenderError::ReadBack(e.to_string()))?;
    rx.recv()
        .map_err(|e| RenderError::ReadBack(e.to_string()))?
        .map_err(|e| RenderError::ReadBack(e.to_string()))?;
    let data = buffer
        .slice(..)
        .get_mapped_range()
        .map_err(|e| RenderError::ReadBack(e.to_string()))?
        .to_vec();
    buffer.unmap();
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(m: &DMat4, ndc_x: f64, ndc_y: f64) -> (f64, f64) {
        let c = *m * DVec4::new(ndc_x, ndc_y, 0.5, 1.0);
        (c.x / c.w, c.y / c.w)
    }

    #[test]
    fn crop_maps_the_window_onto_clip_space() {
        let vp = Viewport::new(800.0, 600.0);
        let r = 7u32;
        let m = pick_crop(100, 200, r, vp);
        // centre of pixel (100, 200)
        let ndc = |px: f64, py: f64| (px / 800.0 * 2.0 - 1.0, 1.0 - py / 600.0 * 2.0);
        let (cx, cy) = ndc(100.5, 200.5);
        let (x, y) = project(&m, cx, cy);
        assert!(x.abs() < 1e-12 && y.abs() < 1e-12, "{x} {y}");
        // left / top edge of the window is -1 / +1, right / bottom edge +1 / -1
        let (lx, ty) = ndc(100.0 - 7.0, 200.0 - 7.0);
        let (x, y) = project(&m, lx, ty);
        assert!((x + 1.0).abs() < 1e-12 && (y - 1.0).abs() < 1e-12, "{x} {y}");
        let (rx, by) = ndc(100.0 + 8.0, 200.0 + 8.0);
        let (x, y) = project(&m, rx, by);
        assert!((x - 1.0).abs() < 1e-12 && (y + 1.0).abs() < 1e-12, "{x} {y}");
        // z and w are untouched
        let c = m * DVec4::new(0.1, 0.2, 0.7, 2.0);
        assert!((c.z - 0.7).abs() < 1e-12 && (c.w - 2.0).abs() < 1e-12);
    }

    #[test]
    fn crop_of_radius_zero_is_a_single_pixel() {
        let vp = Viewport::new(10.0, 10.0);
        let m = pick_crop(3, 4, 0, vp);
        let ndc = |px: f64, py: f64| (px / 10.0 * 2.0 - 1.0, 1.0 - py / 10.0 * 2.0);
        let (x0, y0) = ndc(3.0, 4.0);
        let (x, y) = project(&m, x0, y0);
        assert!((x + 1.0).abs() < 1e-12 && (y - 1.0).abs() < 1e-12);
    }

    #[test]
    fn row_alignment() {
        assert_eq!(align256(1), 256);
        assert_eq!(align256(256), 256);
        assert_eq!(align256(257), 512);
        assert_eq!(align256(15 * 8), 256);
    }
}
