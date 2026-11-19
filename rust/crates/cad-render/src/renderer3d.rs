//! `Renderer3d`: state of the 3D view (model, styles, selection, section, overlays, camera) and its GPU resources.
//! Drawing lives in `frame.rs`, picking in `pickpass.rs`, screenshots in `capture.rs`.
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use cad_types::{Aabb, EntityKind, EntityRef, ModelTree, NodeId, ProtoMesh, TreeNode};
use eframe::{egui, egui_wgpu, wgpu};
use glam::DVec3;

use crate::camera::{Camera, CameraAnimation, StdView, Viewport, FIT_ALL_MARGIN, FIT_SELECTION_MARGIN};
use crate::colors::{
    pack_rgba, Rgb8, DEFAULT_PART_COLOR, EDGE_COLOR, HIGHLIGHT_COLOR, PICK_COLOR, SELECTION_COLOR,
};
use crate::gpu::{block_on, RenderError};
use crate::overlay::{OverlayLine, OverlayPoint, Overlays};
use crate::pipelines::{self, Layouts, PickPipelines, Pipelines};
use crate::pick::PickFilter;
use crate::scene3d::{
    build_proto, center_of, collect_instances, instance_bounds, layout_instances, leaves_below, model_relative,
    radius_of, resolve_hidden, union, InstanceInfo, InstanceLayout, InstanceRaw, ProtoData,
};
use crate::text::GlyphAtlas;
use crate::viewcube::{self, Region};

/// Default MSAA sample count (`DEFAULT_MSAA = 2` in the Python viewer).
pub const DEFAULT_MSAA: u32 = 2;

/// MSAA samples from the environment variable `STEPVIEW_MSAA` (0, 2, 4, 8), else [`DEFAULT_MSAA`].
pub fn msaa_from_env() -> u32 {
    msaa_from_value(std::env::var("STEPVIEW_MSAA").ok().as_deref())
}

/// `msaa_samples()` of the Python viewer for an already read value.
pub fn msaa_from_value(v: Option<&str>) -> u32 {
    match v.and_then(|s| s.trim().parse::<u32>().ok()) {
        Some(n) if matches!(n, 0 | 2 | 4 | 8) => n,
        _ => DEFAULT_MSAA,
    }
}

/// Display options of the 3D view.
#[derive(Clone, Debug, PartialEq)]
pub struct Style {
    /// B-rep edges drawn over the shaded surfaces ("Hrany").
    pub edges: bool,
    /// Wireframe display ("Drátově"): only the edges, in the colour of the parts.
    pub wireframe: bool,
    /// Dark gradient background (the Python app switches it on together with the wireframe).
    pub dark_background: bool,
    pub scale_bar: bool,
    pub view_cube: bool,
    pub default_color: Rgb8,
    pub edge_color: Rgb8,
    /// Requested MSAA samples: 0 or 1 = off, else 2, 4 or 8 (reduced to what the GPU supports).
    pub msaa: u32,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            edges: true,
            wireframe: false,
            dark_background: false,
            scale_bar: true,
            view_cube: true,
            default_color: DEFAULT_PART_COLOR,
            edge_color: EDGE_COLOR,
            msaa: msaa_from_env(),
        }
    }
}

/// Section plane perpendicular to a coordinate axis (`set_section`): the half-space where the plane normal points
/// is kept; the normal is -1 on the axis, +1 when flipped. The cut is capped.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Section {
    /// 0 = X, 1 = Y, 2 = Z.
    pub axis: usize,
    pub position: f64,
    pub flip: bool,
}

impl Section {
    /// Plane `(normal, d)` with `dot(normal, p) + d >= 0` on the kept side.
    pub fn plane(&self) -> ([f64; 3], f64) {
        let mut n = [0.0; 3];
        let axis = self.axis.min(2);
        n[axis] = if self.flip { 1.0 } else { -1.0 };
        (n, -n[axis] * self.position)
    }

    /// Whether a point is on the kept side.
    pub fn keeps(&self, p: [f64; 3]) -> bool {
        let (n, d) = self.plane();
        n[0] * p[0] + n[1] * p[1] + n[2] * p[2] + d >= 0.0
    }
}

/// Entity or whole part highlighted by the hover.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Highlight {
    Node(NodeId),
    Entity(EntityRef),
}

/// A pick result.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PickHit {
    pub entity: EntityRef,
    /// World position under the cursor (from the depth buffer); exact for vertices.
    pub point: [f64; 3],
    /// Distance of the hit pixel from the cursor in pixels.
    pub pixel_distance: f32,
}

/// Format, sample count and depth format of the egui render pass that the picture is copied into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TargetInfo {
    pub format: wgpu::TextureFormat,
    pub samples: u32,
    pub depth: Option<wgpu::TextureFormat>,
}

/// Growable GPU buffer that is rewritten every frame. It is created under the error scopes of the device: a size the
/// device refuses or a graphics memory that is used up leaves the buffer empty (`failed`, nothing is drawn from it)
/// instead of panicking.
pub(crate) struct DynBuf {
    pub buf: Option<wgpu::Buffer>,
    cap: u64,
    pub len: u64,
    usage: wgpu::BufferUsages,
    label: &'static str,
    /// The last upload could not be made; `take_failure` reports it once.
    pub failed: bool,
    reported: bool,
    /// The device refuses buffers over this size (tests).
    limit: Option<u64>,
}

impl DynBuf {
    pub fn new(label: &'static str, usage: wgpu::BufferUsages) -> Self {
        Self { buf: None, cap: 0, len: 0, usage: usage | wgpu::BufferUsages::COPY_DST, label, failed: false, reported: false, limit: None }
    }

    /// Behave as if the device refused buffers over `limit` bytes (tests of the failure path).
    pub fn set_limit(&mut self, limit: Option<u64>) {
        self.limit = limit;
    }

    /// Writes `bytes` into the buffer (made larger when needed). False when it could not be done: the buffer is then
    /// empty (`len` 0) and nothing may be drawn from it.
    pub fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, bytes: &[u8]) -> bool {
        self.len = bytes.len() as u64;
        if bytes.is_empty() {
            return true;
        }
        if self.buf.is_none() || self.cap < self.len {
            let limit = self.limit.unwrap_or(u64::MAX).min(device.limits().max_buffer_size);
            // a buffer the device would refuse is not asked for: the size is rounded up to a power of two, but never over the
            // limit (a request of 600 MB is 1 GiB as a power of two, and the device takes at most 256 MiB)
            let cap = self.len.next_power_of_two().max(256).min(limit.max(self.len));
            let made = if self.len > limit {
                Err(format!("buffer '{}' of {} bytes is over the limit of the device ({limit} bytes)", self.label, self.len))
            } else {
                crate::gpubuf::guarded(device, self.label, || {
                    device.create_buffer(&wgpu::BufferDescriptor { label: Some(self.label), size: cap, usage: self.usage, mapped_at_creation: false })
                })
            };
            match made {
                Ok(b) => {
                    self.buf = Some(b);
                    self.cap = cap;
                }
                Err(_) => {
                    self.buf = None;
                    self.cap = 0;
                    self.len = 0;
                    self.failed = true;
                    return false;
                }
            }
        }
        if let Some(b) = &self.buf {
            queue.write_buffer(b, 0, bytes);
        }
        self.failed = false;
        self.reported = false;
        true
    }

    /// True once per failure (until an upload succeeds again): the renderer turns it into a Czech warning.
    pub fn take_failure(&mut self) -> bool {
        let first = self.failed && !self.reported;
        if first {
            self.reported = true;
        }
        first
    }
}

/// Vertices and indices of one piece of a prototype (or of the whole prototype when it fits into one buffer). The indices
/// refer to the vertices of the piece; `first` is where the piece starts in the index sequence of the prototype, so that
/// ranges of that sequence (the triangles of a face) are drawn from the pieces they touch.
pub(crate) struct GpuPart {
    pub vbuf: wgpu::Buffer,
    pub ibuf: wgpu::Buffer,
    pub first: u32,
    pub count: u32,
}

impl GpuPart {
    /// Draws the indices `first..first + count` of the whole sequence that lie in this part, for the instances `inst`.
    pub fn draw_range(&self, pass: &mut wgpu::RenderPass<'_>, first: u32, count: u32, inst: std::ops::Range<u32>) {
        let lo = first.max(self.first);
        let hi = first.saturating_add(count).min(self.first + self.count);
        if lo >= hi {
            return;
        }
        pass.set_vertex_buffer(0, self.vbuf.slice(..));
        pass.set_index_buffer(self.ibuf.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(lo - self.first..hi - self.first, 0, inst);
    }
}

/// GPU buffers of one prototype plus the CPU data needed for highlights.
pub(crate) struct ProtoGpu {
    /// Surface triangles in as many pieces as the device needs (`gpubuf`).
    pub parts: Vec<GpuPart>,
    pub index_count: u32,
    /// Edge segments, likewise.
    pub eparts: Vec<GpuPart>,
    pub edge_index_count: u32,
    pub pts_bind: Option<wgpu::BindGroup>,
    pub pts_count: u32,
    pub data: ProtoData,
    /// A buffer could not be created (memory of the graphics card): that part of the prototype is not drawn.
    pub failed: bool,
}

/// The surface (3 indices per triangle) or edge (2 indices per segment) buffers of a prototype: one pair of buffers when
/// the data fits `chunk` bytes, else pieces of at most that size. A pair that cannot be created is left out and sets
/// `failed`.
fn build_parts<V: bytemuck::Pod>(
    device: &wgpu::Device,
    vertices: &[V],
    indices: &[u32],
    stride: usize,
    chunk: u64,
    limit: Option<u64>,
    failed: &mut bool,
) -> Vec<GpuPart> {
    use crate::gpubuf::{split_indexed, try_buffer_limited};
    if vertices.is_empty() || indices.is_empty() {
        return Vec::new();
    }
    let max_v = (chunk / std::mem::size_of::<V>() as u64) as usize;
    let max_i = (chunk / 4) as usize;
    let mut make = |v: &[V], i: &[u32], first: usize| -> Option<GpuPart> {
        let vbuf = try_buffer_limited(device, "proto vertices", bytemuck::cast_slice(v), wgpu::BufferUsages::VERTEX, limit);
        let ibuf = try_buffer_limited(device, "proto indices", bytemuck::cast_slice(i), wgpu::BufferUsages::INDEX, limit);
        match (vbuf, ibuf) {
            (Ok(vbuf), Ok(ibuf)) => Some(GpuPart { vbuf, ibuf, first: first as u32, count: i.len() as u32 }),
            _ => {
                *failed = true;
                None
            }
        }
    };
    if vertices.len() <= max_v && indices.len() <= max_i {
        return make(vertices, indices, 0).into_iter().collect();
    }
    split_indexed(vertices, indices, stride, max_v, max_i).into_iter().filter_map(|p| make(&p.vertices, &p.indices, p.first_index)).collect()
}

impl ProtoGpu {
    /// Buffers of at most `chunk` bytes each (the largest buffer a device takes is 256 MiB: a mesh of 8 million vertices
    /// of 32 bytes is as big). A buffer that cannot be created is left out and `failed` is set; nothing panics.
    pub fn upload(device: &wgpu::Device, layouts: &Layouts, mut data: ProtoData, chunk: u64, limit: Option<u64>) -> Self {
        use crate::gpubuf::try_buffer_limited;
        let mut failed = false;
        let parts = build_parts(device, &data.vertices, &data.indices, 3, chunk, limit, &mut failed);
        let eparts = build_parts(device, &data.edge_vertices, &data.edge_indices, 2, chunk, limit, &mut failed);
        let pts: Vec<[f32; 4]> = data.points.iter().map(|p| [p[0], p[1], p[2], 0.0]).collect();
        let pts_bind = if pts.is_empty() {
            None
        } else {
            match try_buffer_limited(device, "proto vertices (points)", bytemuck::cast_slice(&pts), wgpu::BufferUsages::STORAGE, limit) {
                Ok(b) => Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("points"),
                    layout: &layouts.vpick,
                    entries: &[wgpu::BindGroupEntry { binding: 0, resource: b.as_entire_binding() }],
                })),
                Err(_) => {
                    failed = true;
                    None
                }
            }
        };
        let index_count = data.indices.len() as u32;
        let edge_index_count = data.edge_indices.len() as u32;
        let pts_count = if pts_bind.is_some() { data.points.len() as u32 } else { 0 };
        // the triangle data lives on the GPU now; keep only what highlights need
        data.vertices = Vec::new();
        data.indices = Vec::new();
        data.edge_vertices = Vec::new();
        data.edge_indices = Vec::new();
        Self { parts, index_count, eparts, edge_index_count, pts_bind, pts_count, data, failed }
    }
}

/// Offscreen picture with depth and optional MSAA colour.
pub(crate) struct FrameTarget {
    pub width: u32,
    pub height: u32,
    pub samples: u32,
    pub color: wgpu::Texture,
    pub color_view: wgpu::TextureView,
    pub msaa_view: Option<wgpu::TextureView>,
    pub depth_view: wgpu::TextureView,
    _msaa: Option<wgpu::Texture>,
    _depth: wgpu::Texture,
}

impl FrameTarget {
    pub fn new(device: &wgpu::Device, width: u32, height: u32, samples: u32) -> Self {
        let size = wgpu::Extent3d { width: width.max(1), height: height.max(1), depth_or_array_layers: 1 };
        let tex = |label: &str, format, count, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size,
                mip_level_count: 1,
                sample_count: count,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let color = tex(
            "view color",
            pipelines::COLOR_FORMAT,
            1,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
        );
        let msaa = (samples > 1).then(|| {
            tex("view color msaa", pipelines::COLOR_FORMAT, samples, wgpu::TextureUsages::RENDER_ATTACHMENT)
        });
        let depth = tex("view depth", pipelines::SCENE_DEPTH_FORMAT, samples.max(1), wgpu::TextureUsages::RENDER_ATTACHMENT);
        Self {
            width: size.width,
            height: size.height,
            samples: samples.max(1),
            color_view: color.create_view(&wgpu::TextureViewDescriptor::default()),
            msaa_view: msaa.as_ref().map(|t| t.create_view(&wgpu::TextureViewDescriptor::default())),
            depth_view: depth.create_view(&wgpu::TextureViewDescriptor::default()),
            color,
            _msaa: msaa,
            _depth: depth,
        }
    }
}

/// Largest supported sample count not above `requested` (0 and 1 mean off).
pub fn probe_sample_count(device: &wgpu::Device, requested: u32) -> u32 {
    let mut n = match requested {
        0 | 1 => return 1,
        2 => 2,
        3..=4 => 4,
        _ => 8,
    };
    loop {
        if n <= 1 {
            return 1;
        }
        let guard = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mk = |format| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some("msaa probe"),
                size: wgpu::Extent3d { width: 4, height: 4, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: n,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
        };
        let _c = mk(pipelines::COLOR_FORMAT);
        let _d = mk(pipelines::SCENE_DEPTH_FORMAT);
        if block_on(guard.pop()).is_none() {
            return n;
        }
        n /= 2;
    }
}

/// Instance state kept per leaf besides what the tree says.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct InstState {
    /// 0 = opaque, 0.75 = "Průhledné".
    pub transparency: f32,
}

/// Highlight face drawn from the GPU buffers of a prototype.
#[derive(Clone, Copy, Debug)]
pub(crate) struct HlFace {
    pub proto: usize,
    pub first: u32,
    pub count: u32,
    pub raw: InstanceRaw,
}

/// Prepared overlay mesh.
pub(crate) struct OverlayMeshGpu {
    pub gpu: ProtoGpu,
    pub raw: InstanceRaw,
}

pub struct Renderer3d {
    pub(crate) device: wgpu::Device,
    pub(crate) queue: Option<wgpu::Queue>,
    pub(crate) layouts: Layouts,
    pub(crate) pipelines: Pipelines,
    pub(crate) pick_pipes: PickPipelines,
    pub(crate) blit_pipe: wgpu::RenderPipeline,
    pub(crate) target_info: TargetInfo,
    pub(crate) samples: u32,
    pub(crate) blit_sampler: wgpu::Sampler,
    pub(crate) blit_bind: Option<wgpu::BindGroup>,
    pub(crate) screen_target: Option<FrameTarget>,

    pub(crate) atlas: Option<GlyphAtlas>,
    pub(crate) atlas_tex: wgpu::Texture,
    pub(crate) atlas_bind: wgpu::BindGroup,

    pub(crate) globals_buf: wgpu::Buffer,
    pub(crate) globals_bind: wgpu::BindGroup,
    pub(crate) pick_globals_buf: wgpu::Buffer,
    pub(crate) pick_globals_bind: wgpu::BindGroup,

    // ---- model
    pub(crate) protos: Vec<ProtoGpu>,
    pub(crate) nodes: Vec<TreeNode>,
    pub(crate) instances: Vec<InstanceInfo>,
    pub(crate) inst_state: Vec<InstState>,
    pub(crate) node_instance: HashMap<NodeId, usize>,
    pub(crate) hidden_nodes: HashSet<NodeId>,
    pub(crate) hidden_inst: Vec<bool>,
    pub(crate) selected: HashSet<usize>,
    pub(crate) hover_inst: HashSet<usize>,
    pub(crate) layout: InstanceLayout,
    pub(crate) raws: Vec<InstanceRaw>,
    pub(crate) inst_buf: DynBuf,
    pub(crate) layout_dirty: bool,
    pub(crate) dirty_inst: HashSet<usize>,
    pub(crate) origin: DVec3,
    pub(crate) all_bounds: Option<Aabb>,

    // ---- display state
    pub style: Style,
    pub(crate) section: Option<Section>,
    pub(crate) overlays: Overlays,
    pub(crate) overlay_meshes: Vec<OverlayMeshGpu>,
    pub(crate) hover: Option<Highlight>,
    pub(crate) picked: Vec<EntityRef>,
    pub(crate) hl_faces: Vec<HlFace>,
    pub(crate) hl_overlay: Overlays,
    pub(crate) measuring: bool,
    pub(crate) cube_hover: Option<Region>,
    pub(crate) anim: Option<(CameraAnimation, f64)>,

    pub camera: Camera,
    pub(crate) viewport: Viewport,
    pub(crate) dpr: f64,

    // per-frame dynamic buffers
    pub(crate) line_buf: DynBuf,
    pub(crate) point_buf: DynBuf,
    pub(crate) screen_buf: DynBuf,
    pub(crate) ov_inst_buf: DynBuf,
    /// Overlay lines and markers converted once (they can be huge: sharp edges of meshes) and kept on the GPU.
    pub(crate) static_lines: DynBuf,
    pub(crate) static_points: DynBuf,
    pub(crate) static_dpr: Option<f64>,
    pub(crate) hl_dirty: bool,
    /// Set when a buffer of the model could not be created (`take_warning`).
    pub(crate) gpu_warning: Option<String>,
    /// Smaller buffers than the device would allow (tests).
    pub(crate) chunk_bytes_limit: Option<u64>,
    /// The device refuses buffers over this size (tests of the warning).
    pub(crate) simulated_buffer_limit: Option<u64>,
}

impl Renderer3d {
    /// Creates the renderer for the colour format of the egui render pass (`eframe`'s `target_format`). The queue is
    /// attached on the first painted frame, or earlier with [`Renderer3d::with_queue`] (needed for picking and
    /// captures before anything was painted).
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        Self::with_target(device, TargetInfo { format, samples: 1, depth: None })
    }

    /// Like [`Renderer3d::new`] for an egui pass with MSAA or a depth attachment.
    pub fn with_target(device: &wgpu::Device, target_info: TargetInfo) -> Self {
        let layouts = Layouts::new(device);
        let style = Style::default();
        let samples = probe_sample_count(device, style.msaa);
        let pipelines = Pipelines::new(device, &layouts, samples);
        let pick_pipes = PickPipelines::new(device, &layouts);
        let blit_pipe =
            pipelines::blit_pipeline(device, &layouts, target_info.format, target_info.samples, target_info.depth);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("linear clamp"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        let blit_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("nearest clamp"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let atlas_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("glyph atlas"),
            size: wgpu::Extent3d {
                width: crate::text::ATLAS_SIZE,
                height: crate::text::ATLAS_SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let atlas_view = atlas_tex.create_view(&wgpu::TextureViewDescriptor::default());
        let atlas_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("atlas"),
            layout: &layouts.atlas,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&atlas_view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) },
            ],
        });
        let mk_globals = |label: &str| {
            let buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: std::mem::size_of::<crate::gpu_types::Globals>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &layouts.globals,
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: buf.as_entire_binding() }],
            });
            (buf, bind)
        };
        let (globals_buf, globals_bind) = mk_globals("globals");
        let (pick_globals_buf, pick_globals_bind) = mk_globals("pick globals");
        Self {
            device: device.clone(),
            queue: None,
            layouts,
            pipelines,
            pick_pipes,
            blit_pipe,
            target_info,
            samples,
            blit_sampler,
            blit_bind: None,
            screen_target: None,
            atlas: GlyphAtlas::new(),
            atlas_tex,
            atlas_bind,
            globals_buf,
            globals_bind,
            pick_globals_buf,
            pick_globals_bind,
            protos: Vec::new(),
            nodes: Vec::new(),
            instances: Vec::new(),
            inst_state: Vec::new(),
            node_instance: HashMap::new(),
            hidden_nodes: HashSet::new(),
            hidden_inst: Vec::new(),
            selected: HashSet::new(),
            hover_inst: HashSet::new(),
            layout: InstanceLayout::default(),
            raws: Vec::new(),
            inst_buf: DynBuf::new("instances", wgpu::BufferUsages::VERTEX),
            layout_dirty: true,
            dirty_inst: HashSet::new(),
            origin: DVec3::ZERO,
            all_bounds: None,
            style,
            section: None,
            overlays: Overlays::default(),
            overlay_meshes: Vec::new(),
            hover: None,
            picked: Vec::new(),
            hl_faces: Vec::new(),
            hl_overlay: Overlays::default(),
            measuring: false,
            cube_hover: None,
            anim: None,
            camera: Camera::default(),
            viewport: Viewport::new(800.0, 600.0),
            dpr: 1.0,
            line_buf: DynBuf::new("lines", wgpu::BufferUsages::VERTEX),
            point_buf: DynBuf::new("points", wgpu::BufferUsages::VERTEX),
            screen_buf: DynBuf::new("screen", wgpu::BufferUsages::VERTEX),
            ov_inst_buf: DynBuf::new("overlay instances", wgpu::BufferUsages::VERTEX),
            static_lines: DynBuf::new("static overlay lines", wgpu::BufferUsages::VERTEX),
            static_points: DynBuf::new("static overlay points", wgpu::BufferUsages::VERTEX),
            static_dpr: None,
            hl_dirty: false,
            gpu_warning: None,
            chunk_bytes_limit: None,
            simulated_buffer_limit: None,
        }
    }

    /// Convenience for eframe: device, queue and target format of the render state.
    pub fn from_render_state(rs: &egui_wgpu::RenderState) -> Self {
        Self::new(&rs.device, rs.target_format).with_queue(&rs.queue)
    }

    /// Wraps the renderer for use in paint callbacks.
    pub fn shared(self) -> Arc<Mutex<Renderer3d>> {
        Arc::new(Mutex::new(self))
    }

    pub fn with_queue(mut self, queue: &wgpu::Queue) -> Self {
        self.queue = Some(queue.clone());
        self
    }

    pub fn set_queue(&mut self, queue: &wgpu::Queue) {
        self.queue = Some(queue.clone());
    }

    /// Format, sample count and depth format of the egui pass the picture is copied into.
    pub fn target_info(&self) -> TargetInfo {
        self.target_info
    }

    /// Effective MSAA sample count (what the GPU supports).
    pub fn msaa_samples(&self) -> u32 {
        self.samples
    }

    // ------------------------------------------------------------------ model

    /// Bytes one buffer of the model may have: what the device allows (`gpubuf::chunk_bytes`), or less in tests.
    fn chunk_bytes(&self) -> u64 {
        self.chunk_bytes_limit.unwrap_or(u64::MAX).min(crate::gpubuf::chunk_bytes(&self.device))
    }

    /// Cuts the next models into buffers of at most `bytes` (never more than the device allows): tests of the splitting.
    pub fn set_buffer_chunk_bytes(&mut self, bytes: Option<u64>) {
        self.chunk_bytes_limit = bytes;
    }

    /// Behave as if the device refused buffers over `limit` bytes (tests of the warning and of the partial picture).
    #[cfg(test)]
    pub(crate) fn set_simulated_buffer_limit(&mut self, limit: Option<u64>) {
        self.simulated_buffer_limit = limit;
        self.set_dyn_limit(limit);
    }

    /// The buffers that are written again for each frame refuse sizes over `limit` (tests).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn set_dyn_limit(&mut self, limit: Option<u64>) {
        for b in [&mut self.inst_buf, &mut self.line_buf, &mut self.point_buf, &mut self.screen_buf, &mut self.ov_inst_buf, &mut self.static_lines, &mut self.static_points] {
            b.set_limit(limit);
        }
    }

    /// Czech warning when the last model did not fit into the memory of the graphics card (some of it is not drawn), or
    /// when a buffer of the frame (instances, overlays) could not be made. Taken once.
    pub fn take_warning(&mut self) -> Option<String> {
        if let Some(w) = self.gpu_warning.take() {
            return Some(w);
        }
        let mut dynamic = false;
        for b in [&mut self.inst_buf, &mut self.line_buf, &mut self.point_buf, &mut self.screen_buf, &mut self.ov_inst_buf, &mut self.static_lines, &mut self.static_points] {
            dynamic |= b.take_failure();
        }
        dynamic.then(|| crate::gpubuf::GPU_DYNAMIC_WARNING.to_string())
    }

    /// Number of buffer pairs (surface, edges) the prototypes of the model are held in.
    pub fn buffer_counts(&self) -> (usize, usize) {
        (self.protos.iter().map(|p| p.parts.len()).sum(), self.protos.iter().map(|p| p.eparts.len()).sum())
    }

    /// Replaces the model. `protos[i]` is `ProtoId` i of the tree.
    pub fn set_model(&mut self, protos: &[ProtoMesh], tree: &ModelTree) {
        let chunk = self.chunk_bytes();
        self.protos = protos
            .iter()
            .map(|m| ProtoGpu::upload(&self.device, &self.layouts, build_proto(m), chunk, self.simulated_buffer_limit))
            .collect();
        self.gpu_warning = self.protos.iter().any(|p| p.failed).then(|| crate::gpubuf::GPU_BUFFER_WARNING.to_string());
        self.nodes = tree.nodes.clone();
        self.instances = collect_instances(tree, self.protos.len());
        self.inst_state = vec![InstState::default(); self.instances.len()];
        self.node_instance = self.instances.iter().enumerate().map(|(i, inst)| (inst.node, i)).collect();
        self.selected.clear();
        self.hover_inst.clear();
        self.hidden_nodes.clear();
        self.hover = None;
        self.picked.clear();
        self.hl_faces.clear();
        self.hl_overlay.clear();
        self.overlays.clear();
        self.overlay_meshes.clear();
        self.static_dpr = None;
        let mut bounds = None;
        for inst in &self.instances {
            if let Some(b) = self.protos[inst.proto as usize].data.bounds {
                bounds = union(bounds, &instance_bounds(&inst.model, &b));
            }
        }
        self.all_bounds = bounds;
        self.origin = bounds.map(|b| center_of(&b)).unwrap_or(DVec3::ZERO);
        self.refresh_hidden();
        self.layout_dirty = true;
        self.hl_dirty = true;
    }

    /// Removes the model (new empty scene).
    pub fn clear_model(&mut self) {
        self.set_model(&[], &ModelTree::default());
    }

    pub fn instance_count(&self) -> usize {
        self.instances.len()
    }

    /// Bounds of all leaves, hidden or not.
    pub fn model_bounds(&self) -> Option<Aabb> {
        self.all_bounds
    }

    /// Bounds of the visible leaves (what `fit_all` frames).
    pub fn visible_bounds(&self) -> Option<Aabb> {
        let mut b = None;
        for (i, inst) in self.instances.iter().enumerate() {
            if self.hidden_inst.get(i).copied().unwrap_or(false) {
                continue;
            }
            if let Some(pb) = self.protos[inst.proto as usize].data.bounds {
                b = union(b, &instance_bounds(&inst.model, &pb));
            }
        }
        b
    }

    /// Bounds of the leaves below the given nodes.
    pub fn nodes_bounds(&self, nodes: &[NodeId]) -> Option<Aabb> {
        let mut b = None;
        for &n in nodes {
            for leaf in leaves_below(&self.nodes, n) {
                if let Some(&i) = self.node_instance.get(&leaf) {
                    let inst = &self.instances[i];
                    if let Some(pb) = self.protos[inst.proto as usize].data.bounds {
                        b = union(b, &instance_bounds(&inst.model, &pb));
                    }
                }
            }
        }
        b
    }

    fn refresh_hidden(&mut self) {
        let by_node = resolve_hidden(&self.nodes, &self.hidden_nodes);
        self.hidden_inst = self
            .instances
            .iter()
            .map(|i| by_node.get(i.node as usize).copied().unwrap_or(false))
            .collect();
    }

    /// Hides the given nodes (an assembly hides everything below it); replaces the previous set.
    pub fn set_visibility(&mut self, hidden: &HashSet<NodeId>) {
        self.hidden_nodes = hidden.clone();
        self.refresh_hidden();
        self.layout_dirty = true;
    }

    pub fn hidden_nodes(&self) -> &HashSet<NodeId> {
        &self.hidden_nodes
    }

    /// Display style: edges, wireframe, background, scale bar, colours, MSAA.
    pub fn set_style(&mut self, style: &Style) {
        let msaa_changed = style.msaa != self.style.msaa;
        self.style = style.clone();
        if msaa_changed {
            let samples = probe_sample_count(&self.device, style.msaa);
            if samples != self.samples {
                self.samples = samples;
                self.pipelines = Pipelines::new(&self.device, &self.layouts, samples);
            }
        }
    }

    /// Transparency of nodes in 0..1 (the "T" key uses 0.75; 0 = opaque). An assembly applies to its leaves.
    pub fn set_transparency(&mut self, nodes: &[NodeId], transparency: f32) {
        let t = transparency.clamp(0.0, 1.0);
        for &n in nodes {
            for leaf in leaves_below(&self.nodes, n) {
                if let Some(&i) = self.node_instance.get(&leaf) {
                    if (self.inst_state[i].transparency - t).abs() > f32::EPSILON {
                        self.inst_state[i].transparency = t;
                        self.layout_dirty = true;
                    }
                }
            }
        }
    }

    /// Transparency of a leaf.
    pub fn transparency(&self, node: NodeId) -> f32 {
        self.node_instance.get(&node).map(|&i| self.inst_state[i].transparency).unwrap_or(0.0)
    }

    fn leaf_set(&self, nodes: &[NodeId]) -> HashSet<usize> {
        nodes
            .iter()
            .flat_map(|&n| leaves_below(&self.nodes, n))
            .filter_map(|leaf| self.node_instance.get(&leaf).copied())
            .collect()
    }

    /// Selected parts, drawn in the selection colour. Assemblies select everything below.
    pub fn set_selection(&mut self, nodes: &[NodeId]) {
        let new = self.leaf_set(nodes);
        for &i in new.symmetric_difference(&self.selected) {
            self.dirty_inst.insert(i);
        }
        self.selected = new;
    }

    /// Hover highlight: a whole part (light blue) or one entity (measuring).
    pub fn set_hover(&mut self, hover: Option<Highlight>) {
        if hover == self.hover {
            return;
        }
        self.hover = hover;
        let new: HashSet<usize> = match hover {
            Some(Highlight::Node(n)) => self.leaf_set(&[n]),
            _ => HashSet::new(),
        };
        for &i in new.symmetric_difference(&self.hover_inst) {
            self.dirty_inst.insert(i);
        }
        self.hover_inst = new;
        self.hl_dirty = true;
    }

    /// Entities picked by the measure tool, drawn in the pick colour above the model.
    pub fn set_picked(&mut self, picked: &[EntityRef]) {
        if self.picked != picked {
            self.picked = picked.to_vec();
            self.hl_dirty = true;
        }
    }

    /// Section plane with capping; `None` removes it.
    pub fn set_section(&mut self, section: Option<Section>) {
        self.section = section;
    }

    pub fn section(&self) -> Option<Section> {
        self.section
    }

    /// Replaces the overlays (measurement lines, markers, labels, translucent meshes).
    pub fn set_overlays(&mut self, overlays: Overlays) {
        let chunk = self.chunk_bytes();
        self.overlay_meshes = overlays
            .meshes
            .iter()
            .map(|m| {
                let positions: Vec<[f32; 3]> = m
                    .positions
                    .iter()
                    .map(|p| (DVec3::from_array(*p) - self.origin).as_vec3().to_array())
                    .collect();
                let n = (m.indices.len() / 3 * 3) as u32;
                let mesh = ProtoMesh {
                    positions,
                    indices: m.indices[..n as usize].to_vec(),
                    faces: vec![cad_types::FaceSpan { first_index: 0, index_count: n, color: None }],
                    ..ProtoMesh::default()
                };
                let mut raw = InstanceRaw::default();
                raw.model = glam::Mat4::IDENTITY.to_cols_array_2d();
                raw.tint = pack_rgba(m.color, 255);
                raw.opacity = (m.alpha.clamp(0.0, 1.0) * 255.0).round() as u32;
                OverlayMeshGpu { gpu: ProtoGpu::upload(&self.device, &self.layouts, build_proto(&mesh), chunk, self.simulated_buffer_limit), raw }
            })
            .collect();
        self.overlays = overlays;
        self.static_dpr = None;
    }

    pub fn overlays(&self) -> &Overlays {
        &self.overlays
    }

    pub fn clear_overlays(&mut self) {
        self.set_overlays(Overlays::default());
    }

    /// Measure mode: edges and vertices become pickable with the 7 px tolerance.
    pub fn set_measure_mode(&mut self, on: bool) {
        self.measuring = on;
    }

    pub fn measure_mode(&self) -> bool {
        self.measuring
    }

    // ------------------------------------------------------------------ camera

    pub fn viewport(&self) -> Viewport {
        self.viewport
    }

    pub fn dpr(&self) -> f64 {
        self.dpr
    }

    /// Size of the drawing area in physical pixels and the device pixel ratio; normally set by the paint callback.
    pub fn set_viewport(&mut self, width: u32, height: u32, dpr: f64) {
        self.viewport = Viewport::new(f64::from(width), f64::from(height));
        self.dpr = dpr.max(0.1);
    }

    /// `FitAll(0.04)` of the visible parts.
    pub fn fit_all(&mut self) {
        if let Some(b) = self.visible_bounds().or(self.all_bounds) {
            self.camera.fit_box(&b, FIT_ALL_MARGIN, self.viewport);
        }
    }

    /// Zoom to a box with the selection margin of 0.15.
    pub fn fit_box(&mut self, bounds: &Aabb) {
        self.camera.fit_box(bounds, FIT_SELECTION_MARGIN, self.viewport);
    }

    /// Zoom to the given nodes (`zoom_selected`).
    pub fn fit_nodes(&mut self, nodes: &[NodeId]) {
        if let Some(b) = self.nodes_bounds(nodes) {
            self.fit_box(&b);
        }
    }

    /// Standard view followed by fit-all.
    pub fn set_orientation(&mut self, view: StdView) {
        self.camera.set_view(view);
        self.fit_all();
    }

    pub fn set_perspective(&mut self, on: bool) {
        self.camera.set_perspective(on, self.viewport);
    }

    // ------------------------------------------------------------------ view cube

    /// Region of the view cube under a pixel (physical pixels, origin upper left of the view).
    pub fn view_cube_hit(&self, x: f64, y: f64) -> Option<Region> {
        if !self.style.view_cube {
            return None;
        }
        viewcube::hit_test(&self.camera, &viewcube::CubeLayout::new(self.viewport, self.dpr), [x, y])
    }

    pub fn set_view_cube_hover(&mut self, region: Option<Region>) {
        self.cube_hover = region;
    }

    /// Click on a cube region: animates to that view and fits the model (0.35 s).
    pub fn start_view_animation(&mut self, region: Region) {
        let Some(dir) = viewcube::region_eye_direction(region) else { return };
        let mut end = self.camera;
        end.set_direction(dir, viewcube::region_up(region));
        if let Some(b) = self.visible_bounds().or(self.all_bounds) {
            end.fit_box(&b, FIT_ALL_MARGIN, self.viewport);
        }
        self.anim = Some((CameraAnimation::new(self.camera, end, viewcube::ANIMATION_SECS), 0.0));
    }

    /// Advances a running view animation; returns true while it is still running (keep repainting).
    pub fn advance_animation(&mut self, dt: f64) -> bool {
        let Some((anim, t)) = self.anim.as_mut() else { return false };
        *t += dt.max(0.0);
        self.camera = anim.sample(*t);
        if anim.finished(*t) {
            self.anim = None;
            return false;
        }
        true
    }

    pub fn is_animating(&self) -> bool {
        self.anim.is_some()
    }

    // ------------------------------------------------------------------ highlights

    fn tint_of(&self, i: usize) -> u32 {
        if self.selected.contains(&i) {
            pack_rgba(SELECTION_COLOR, 255)
        } else if self.hover_inst.contains(&i) {
            pack_rgba(HIGHLIGHT_COLOR, 255)
        } else {
            0
        }
    }

    pub(crate) fn make_raw(&self, i: usize) -> InstanceRaw {
        let inst = &self.instances[i];
        InstanceRaw {
            model: model_relative(&inst.model, self.origin),
            color: inst.color.map(|c| pack_rgba(c, 255)).unwrap_or(0),
            tint: self.tint_of(i),
            id: i as u32 + 1,
            opacity: ((1.0 - self.inst_state[i].transparency) * 255.0).round().clamp(0.0, 255.0) as u32,
        }
    }

    /// Writes changed instance data to the GPU.
    pub(crate) fn flush_instances(&mut self, queue: &wgpu::Queue) {
        if self.layout_dirty {
            let entries: Vec<(u32, bool, bool)> = self
                .instances
                .iter()
                .enumerate()
                .map(|(i, inst)| (inst.proto, self.inst_state[i].transparency > 0.0, self.hidden_inst[i]))
                .collect();
            self.layout = layout_instances(&entries);
            self.raws = self.layout.order.iter().map(|&i| self.make_raw(i)).collect();
            self.inst_buf.upload(&self.device, queue, bytemuck::cast_slice(&self.raws));
            self.layout_dirty = false;
            self.dirty_inst.clear();
        } else if !self.dirty_inst.is_empty() {
            let dirty: Vec<usize> = self.dirty_inst.drain().collect();
            for i in dirty {
                let Some(&slot) = self.layout.slot_of.get(i) else { continue };
                if slot == u32::MAX {
                    continue;
                }
                let raw = self.make_raw(i);
                self.raws[slot as usize] = raw;
                if let Some(b) = &self.inst_buf.buf {
                    queue.write_buffer(b, u64::from(slot) * pipelines::INSTANCE_STRIDE, bytemuck::bytes_of(&raw));
                }
            }
        }
    }

    /// Rebuilds the geometry of entity highlights (hover and picked entities).
    pub(crate) fn rebuild_entity_highlights(&mut self) {
        self.hl_faces.clear();
        self.hl_overlay.clear();
        let mut list: Vec<(EntityRef, Rgb8, f32, bool)> = Vec::new();
        if let Some(Highlight::Entity(e)) = self.hover {
            list.push((e, HIGHLIGHT_COLOR, 0.85, true));
        }
        for e in &self.picked {
            list.push((*e, PICK_COLOR, 0.65, false));
        }
        for (e, color, alpha, hover) in list {
            let Some(&i) = self.node_instance.get(&e.node) else { continue };
            let inst = &self.instances[i];
            let Some(proto) = self.protos.get(inst.proto as usize) else { continue };
            let d = &proto.data;
            match e.kind {
                EntityKind::Face => {
                    let Some(&(first, count)) = d.face_ranges.get(e.index as usize) else { continue };
                    let mut raw = InstanceRaw {
                        model: model_relative(&inst.model, self.origin),
                        tint: pack_rgba(color, 255),
                        opacity: (alpha * 255.0).round() as u32,
                        ..InstanceRaw::default()
                    };
                    raw.id = 0;
                    self.hl_faces.push(HlFace { proto: inst.proto as usize, first, count, raw });
                }
                EntityKind::Edge => {
                    let Some(&(first, count)) = d.edge_spans.get(e.index as usize) else { continue };
                    let pts: Vec<[f64; 3]> = d.edge_points[first as usize..(first + count) as usize]
                        .iter()
                        .map(|p| inst.model.transform_point3(DVec3::new(f64::from(p[0]), f64::from(p[1]), f64::from(p[2]))).to_array())
                        .collect();
                    if pts.len() >= 2 {
                        self.hl_overlay.lines.push(OverlayLine {
                            points: pts,
                            closed: false,
                            color,
                            width: if hover { 3.0 } else { 4.0 },
                            dashed: false,
                        });
                    }
                }
                EntityKind::Vertex => {
                    let Some(p) = d.points.get(e.index as usize) else { continue };
                    let w = inst.model.transform_point3(DVec3::new(f64::from(p[0]), f64::from(p[1]), f64::from(p[2])));
                    self.hl_overlay.points.push(OverlayPoint::ball(w.to_array(), color, 3.0));
                }
            }
        }
        self.hl_dirty = false;
    }

    /// Bounding sphere (centre in world coordinates, radius) used for the depth range.
    pub(crate) fn depth_sphere(&self) -> (DVec3, f64) {
        match &self.all_bounds {
            Some(b) => (center_of(b), radius_of(b)),
            None => (self.camera.center, self.camera.scale.max(1.0)),
        }
    }

    pub(crate) fn pick_filter(&self) -> PickFilter {
        if self.measuring {
            PickFilter::ALL
        } else if self.style.wireframe {
            PickFilter::EDGES
        } else {
            PickFilter::FACES
        }
    }

    pub(crate) fn queue_or_err(&self) -> Result<wgpu::Queue, RenderError> {
        self.queue.clone().ok_or(RenderError::NoQueue)
    }

    // ------------------------------------------------------------------ egui integration

    /// Paint callback for the rectangle `rect` (egui points). Add it with `ui.painter().add(..)`.
    pub fn paint_callback(this: &Arc<Mutex<Renderer3d>>, rect: egui::Rect) -> egui::PaintCallback {
        egui_wgpu::Callback::new_paint_callback(rect, Renderer3dCallback { renderer: Arc::clone(this), rect })
    }
}

/// egui_wgpu callback: renders the scene offscreen in `prepare` and copies it into the egui pass in `paint`.
pub struct Renderer3dCallback {
    renderer: Arc<Mutex<Renderer3d>>,
    rect: egui::Rect,
}

impl egui_wgpu::CallbackTrait for Renderer3dCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        screen: &egui_wgpu::ScreenDescriptor,
        encoder: &mut wgpu::CommandEncoder,
        _resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Ok(mut r) = self.renderer.lock() else { return Vec::new() };
        let vp = egui::epaint::ViewportInPixels::from_points(&self.rect, screen.pixels_per_point, screen.size_in_pixels);
        r.prepare_frame(device, queue, encoder, vp.width_px.max(0) as u32, vp.height_px.max(0) as u32, f64::from(screen.pixels_per_point));
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        _resources: &egui_wgpu::CallbackResources,
    ) {
        let Ok(r) = self.renderer.lock() else { return };
        r.blit(render_pass);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn section_normal_is_minus_one_on_the_axis_and_plus_one_when_flipped() {
        for axis in 0..3 {
            let s = Section { axis, position: 12.5, flip: false };
            let (n, d) = s.plane();
            for k in 0..3 {
                assert_eq!(n[k], if k == axis { -1.0 } else { 0.0 });
            }
            // plane passes through the position: dot(n, p) + d = 0 for p[axis] = position
            let mut p = [3.0, 4.0, 5.0];
            p[axis] = 12.5;
            assert!((n[0] * p[0] + n[1] * p[1] + n[2] * p[2] + d).abs() < 1e-12);
            let f = Section { axis, position: 12.5, flip: true };
            assert_eq!(f.plane().0[axis], 1.0);
        }
    }

    #[test]
    fn section_keeps_the_side_the_normal_points_to() {
        // default normal -X: keeps x <= position (OCCT clips where the plane equation is negative)
        let s = Section { axis: 0, position: 10.0, flip: false };
        assert!(s.keeps([9.0, 100.0, -5.0]));
        assert!(!s.keeps([11.0, 0.0, 0.0]));
        assert!(s.keeps([10.0, 0.0, 0.0]));
        let f = Section { axis: 0, position: 10.0, flip: true };
        assert!(!f.keeps([9.0, 0.0, 0.0]));
        assert!(f.keeps([11.0, 0.0, 0.0]));
        // an out-of-range axis is clamped instead of panicking
        let z = Section { axis: 7, position: 0.0, flip: false };
        assert_eq!(z.plane().0, [0.0, 0.0, -1.0]);
    }

    #[test]
    fn msaa_option_follows_the_python_rules() {
        for v in ["0", "2", "4", "8", " 4 "] {
            assert_eq!(msaa_from_value(Some(v)), v.trim().parse::<u32>().unwrap_or(2), "{v}");
        }
        for v in ["1", "3", "16", "-2", "abc", ""] {
            assert_eq!(msaa_from_value(Some(v)), DEFAULT_MSAA, "{v}");
        }
        assert_eq!(msaa_from_value(None), DEFAULT_MSAA);
        assert_eq!(DEFAULT_MSAA, 2);
    }

    #[test]
    fn default_style_matches_the_application_defaults() {
        let s = Style { msaa: 2, ..Style::default() };
        assert!(s.edges && !s.wireframe && !s.dark_background && s.scale_bar && s.view_cube);
        assert_eq!(s.default_color, [200, 204, 210]);
        assert_eq!(s.edge_color, [25, 25, 30]);
    }
}
