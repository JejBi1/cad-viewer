//! `Renderer2d`: draws the batched `Scene2d` of a DXF / DWG drawing with layer visibility, pan and zoom, overlays
//! (selection, measurement, snap marker) and offscreen captures, through the same egui paint callback scheme as the
//! 3D renderer.
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use cad_types::{DrawBatch, LayerId, Scene2d};
use eframe::{egui, egui_wgpu, wgpu};

use crate::gpubuf::{self, ChunkWriter, Chunked};

/// Vertex of a picture: drawing position relative to the scene origin and texture coordinates.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct PictureVertex {
    pos: [f32; 2],
    uv: [f32; 2],
}

/// A picture of the scene (`cad_types::DrawImage`) waiting for its texture.
struct ScenePicture {
    layer: LayerId,
    after: u32,
    width: u32,
    height: u32,
    rgba: Arc<[u8]>,
    corners: [[f32; 2]; 4],
}

/// A picture uploaded to the GPU; its six vertices start at `first` in the picture vertex buffer.
struct GpuPicture {
    layer: LayerId,
    after: u32,
    first: u32,
    bind: wgpu::BindGroup,
}

use crate::camera::Viewport;
use crate::capture::{clamp_thumbnail_size, png_from_rgba, unpremultiply, Rgba8};
use crate::colors::{pack_rgba, Rgb8, BG_2D_DARK, BG_2D_LIGHT, PAPER_BORDER_2D};
use crate::gpu::RenderError;
use crate::gpu_types::{FillVertex2d, Globals, Globals2d, LineInst, LineInst2d, ScreenVertex, LINE_PIXEL_SPACE};
use crate::overlay2d::Overlays2d;
use crate::pickpass::read_buffer;
use crate::pipelines::{self, Layouts, Scene2dPipelines, UiPipelines, COLOR_FORMAT};
use crate::renderer3d::{msaa_from_env, probe_sample_count, DynBuf, TargetInfo};
use crate::text::{push_polygon, push_text, GlyphAtlas, TextFrame};
use crate::view2d::{Rect2, View2d};

/// How the width of drawing lines is interpreted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum LineWidthMode {
    /// `DrawBatch::width` is in drawing units and scales with the zoom (hairlines stay `min_width_px` wide).
    DrawingUnits,
    /// `DrawBatch::width` is a cosmetic pen width in screen pixels, the same at every zoom level, as the Qt viewer drew
    /// it. 0 is a hairline (`min_width_px`). The default, see `cad_types::DrawBatch::width`.
    #[default]
    ScreenPixels,
}

struct Batch2d {
    layer: LayerId,
    lines: (u32, u32),
    fills: (u32, u32),
    bbox: Option<Rect2>,
}

struct Target2d {
    width: u32,
    height: u32,
    samples: u32,
    color: wgpu::Texture,
    color_view: wgpu::TextureView,
    msaa_view: Option<wgpu::TextureView>,
    _msaa: Option<wgpu::Texture>,
}

impl Target2d {
    fn new(device: &wgpu::Device, width: u32, height: u32, samples: u32) -> Self {
        let size = wgpu::Extent3d { width: width.max(1), height: height.max(1), depth_or_array_layers: 1 };
        let tex = |label: &str, count, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size,
                mip_level_count: 1,
                sample_count: count,
                dimension: wgpu::TextureDimension::D2,
                format: COLOR_FORMAT,
                usage,
                view_formats: &[],
            })
        };
        let color = tex(
            "drawing color",
            1,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
        );
        let msaa = (samples > 1).then(|| tex("drawing color msaa", samples, wgpu::TextureUsages::RENDER_ATTACHMENT));
        Self {
            width: size.width,
            height: size.height,
            samples: samples.max(1),
            color_view: color.create_view(&wgpu::TextureViewDescriptor::default()),
            msaa_view: msaa.as_ref().map(|t| t.create_view(&wgpu::TextureViewDescriptor::default())),
            color,
            _msaa: msaa,
        }
    }
}

/// Options of one rendered picture.
struct Opts2d {
    /// `None` = transparent.
    background: Option<Rgb8>,
    /// White paper with a grey border over this pixel rectangle `[x0, y0, x1, y1]` (thumbnails).
    paper: Option<[f64; 4]>,
    ui: bool,
}

#[derive(Default)]
struct UiLists {
    lines: Vec<LineInst>,
    polys: Vec<ScreenVertex>,
    text: Vec<ScreenVertex>,
}

/// Two colours that no batch of the role 0 has as its colour: the markers of the foreground and of the background batches
/// (`Renderer2d::set_scene_owned`), so that no other batch is changed by mistake.
fn free_marks(batches: &[DrawBatch], roles: &[u8]) -> ([u8; 3], [u8; 3]) {
    let used: HashSet<[u8; 3]> = batches.iter().zip(roles).filter(|(_, r)| **r == 0).map(|(b, _)| [b.color[0], b.color[1], b.color[2]]).collect();
    let mut found: Vec<[u8; 3]> = Vec::new();
    for n in 1u32.. {
        let c = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
        if !used.contains(&c) {
            found.push(c);
            if found.len() == 2 {
                break;
            }
        }
    }
    (found[0], found[1])
}

pub struct Renderer2d {
    device: wgpu::Device,
    queue: Option<wgpu::Queue>,
    layouts: Layouts,
    ui_pipes: UiPipelines,
    scene_pipes: Scene2dPipelines,
    blit_pipe: wgpu::RenderPipeline,
    blit_sampler: wgpu::Sampler,
    blit_bind: Option<wgpu::BindGroup>,
    target_info: TargetInfo,
    samples: u32,
    target: Option<Target2d>,

    atlas: Option<GlyphAtlas>,
    atlas_tex: wgpu::Texture,
    atlas_bind: wgpu::BindGroup,

    g2_buf: wgpu::Buffer,
    g2_bind: wgpu::BindGroup,
    ui_buf: wgpu::Buffer,
    ui_bind: wgpu::BindGroup,

    /// Segments and fill corners of the scene, each in as many buffers as the device needs (`gpubuf`); the batches refer
    /// to ranges of the whole sequence and are drawn in their order.
    lines: Chunked,
    fills: Chunked,
    /// The highlight of the picked entity: segments made once when it is picked (`set_highlight`), in buffers of their own
    /// that are cut like those of the scene. Drawn after the scene, in the coordinates of its origin.
    hl_lines: Chunked,
    hl_g2_buf: wgpu::Buffer,
    hl_g2_bind: wgpu::BindGroup,
    /// Which batches of the scene are drawn (`set_batch_mask`); `None` = all of them (subject to their layer).
    batch_mask: Option<Vec<bool>>,
    /// The marker colours that the foreground and background batches of the scene were uploaded with, and the colours drawn
    /// in their place now (`set_scene_owned`, `set_palette`).
    marks: Option<([u8; 3], [u8; 3])>,
    palette: ([u8; 3], [u8; 3]),
    /// Set when a buffer could not be created (`take_warning`).
    gpu_warning: Option<String>,
    /// Smaller buffers than the device would allow (tests).
    chunk_bytes_limit: Option<u64>,
    /// The device refuses buffers over this size (tests of the warning).
    simulated_buffer_limit: Option<u64>,
    batches: Vec<Batch2d>,
    layer_visible: HashMap<LayerId, bool>,
    /// World point the scene coordinates are relative to (`Scene2d::origin`).
    origin: [f64; 2],
    bounds: Option<Rect2>,
    scene_pictures: Vec<ScenePicture>,
    /// Pictures uploaded for the current scene; built on the first frame that has a queue.
    gpu_pictures: Vec<GpuPicture>,
    gpu_pictures_ready: bool,
    picture_buf: Option<wgpu::Buffer>,
    picture_sampler: wgpu::Sampler,

    /// Pan and zoom.
    pub view: View2d,
    viewport: Viewport,
    dpr: f64,
    /// Background colour: [`BG_2D_DARK`] (ezdxf default) or [`BG_2D_LIGHT`] for "bílé pozadí".
    pub background: Rgb8,
    pub width_mode: LineWidthMode,
    /// Narrowest line drawn in pixels.
    pub min_width_px: f32,
    overlays: Overlays2d,

    ov_lines: DynBuf,
    ov_screen: DynBuf,
    paper_lines: DynBuf,
    paper_screen: DynBuf,
}

impl Renderer2d {
    /// Creates the renderer for the colour format of the egui render pass.
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        Self::with_target(device, TargetInfo { format, samples: 1, depth: None })
    }

    pub fn with_target(device: &wgpu::Device, target_info: TargetInfo) -> Self {
        let layouts = Layouts::new(device);
        let samples = probe_sample_count(device, msaa_from_env());
        let ui_pipes = UiPipelines::new(device, &layouts, samples);
        let scene_pipes = Scene2dPipelines::new(device, &layouts, samples);
        let blit_pipe =
            pipelines::blit_pipeline(device, &layouts, target_info.format, target_info.samples, target_info.depth);
        let nearest = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("nearest clamp"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let linear = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("linear clamp"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let atlas_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("glyph atlas 2d"),
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
            label: Some("atlas 2d"),
            layout: &layouts.atlas,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&atlas_view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&linear) },
            ],
        });
        let uniform = |label: &str, size: usize| {
            let buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: size as u64,
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
        let (g2_buf, g2_bind) = uniform("globals 2d", std::mem::size_of::<Globals2d>());
        let (ui_buf, ui_bind) = uniform("ui globals", std::mem::size_of::<Globals>());
        let (hl_g2_buf, hl_g2_bind) = uniform("globals 2d highlight", std::mem::size_of::<Globals2d>());
        Self {
            device: device.clone(),
            queue: None,
            layouts,
            ui_pipes,
            scene_pipes,
            blit_pipe,
            blit_sampler: nearest,
            blit_bind: None,
            target_info,
            samples,
            target: None,
            atlas: GlyphAtlas::new(),
            atlas_tex,
            atlas_bind,
            g2_buf,
            g2_bind,
            ui_buf,
            ui_bind,
            lines: Chunked::default(),
            fills: Chunked::default(),
            hl_lines: Chunked::default(),
            hl_g2_buf,
            hl_g2_bind,
            batch_mask: None,
            marks: None,
            palette: ([0, 0, 0], BG_2D_LIGHT),
            gpu_warning: None,
            chunk_bytes_limit: None,
            simulated_buffer_limit: None,
            batches: Vec::new(),
            layer_visible: HashMap::new(),
            origin: [0.0, 0.0],
            bounds: None,
            scene_pictures: Vec::new(),
            gpu_pictures: Vec::new(),
            gpu_pictures_ready: false,
            picture_buf: None,
            picture_sampler: linear,
            view: View2d::default(),
            viewport: Viewport::new(800.0, 600.0),
            dpr: 1.0,
            background: BG_2D_DARK,
            width_mode: LineWidthMode::default(),
            min_width_px: 1.0,
            overlays: Overlays2d::default(),
            ov_lines: DynBuf::new("2d overlay lines", wgpu::BufferUsages::VERTEX),
            ov_screen: DynBuf::new("2d overlay screen", wgpu::BufferUsages::VERTEX),
            paper_lines: DynBuf::new("2d paper lines", wgpu::BufferUsages::VERTEX),
            paper_screen: DynBuf::new("2d paper screen", wgpu::BufferUsages::VERTEX),
        }
    }

    pub fn from_render_state(rs: &egui_wgpu::RenderState) -> Self {
        Self::new(&rs.device, rs.target_format).with_queue(&rs.queue)
    }

    pub fn shared(self) -> Arc<Mutex<Renderer2d>> {
        Arc::new(Mutex::new(self))
    }

    pub fn with_queue(mut self, queue: &wgpu::Queue) -> Self {
        self.queue = Some(queue.clone());
        self
    }

    pub fn set_queue(&mut self, queue: &wgpu::Queue) {
        self.queue = Some(queue.clone());
    }

    pub fn target_info(&self) -> TargetInfo {
        self.target_info
    }

    /// Background of the picture: white for the "bílé pozadí" option, else the ezdxf model space colour.
    pub fn set_light_background(&mut self, light: bool) {
        self.background = if light { BG_2D_LIGHT } else { BG_2D_DARK };
    }

    // ------------------------------------------------------------------ scene

    /// Replaces the drawing and fits it into the view (`set_scene`).
    pub fn set_scene(&mut self, scene: &Scene2d) {
        self.upload_scene(scene);
        self.fit();
    }

    /// Replaces the drawing and keeps pan and zoom (`set_scene(keep_view=True)`).
    pub fn set_scene_keep_view(&mut self, scene: &Scene2d) {
        self.upload_scene(scene);
    }

    /// `set_scene` / `set_scene_keep_view` for a scene that is not needed any more afterwards: its batches are freed one by
    /// one as they are copied to the graphics card, so the program never holds the whole scene twice (a scene of 100
    /// million vertices is 800 MB in memory and as much on the card). `mask` says which batches are drawn (`set_batch_mask`),
    /// before the view is fitted to what is shown.
    ///
    /// `roles` has one entry per batch: 1 for a batch whose colour is the foreground of the layout, 2 for the background, 0 for
    /// every other colour (an empty list: all 0). Those batches are drawn in the colours of `palette` = (foreground,
    /// background), which `set_palette` changes later without a new scene. The background of the picture is the palette's.
    pub fn set_scene_owned(&mut self, mut scene: Scene2d, mask: &[bool], roles: &[u8], palette: Option<([u8; 3], [u8; 3])>, keep_view: bool) {
        let roles = if palette.is_some() && roles.len() == scene.batches.len() { roles } else { &[] };
        // markers that no other batch has as its colour, so that nothing else is changed by mistake
        self.marks = if roles.iter().any(|r| *r != 0) { Some(free_marks(&scene.batches, roles)) } else { None };
        if let Some(p) = palette {
            self.palette = p;
            self.background = p.1;
        }
        let batches = std::mem::take(&mut scene.batches);
        self.upload_scene_from(&scene, batches.into_iter().map(Cow::Owned), roles);
        self.batch_mask = (mask.len() == self.batches.len()).then(|| mask.to_vec());
        if !keep_view {
            self.fit();
        }
    }

    /// The colours that the foreground and the background batches of the scene are drawn in, and the background of the
    /// picture (white paper or the dark model space), changed without a new scene.
    pub fn set_palette(&mut self, foreground: [u8; 3], background: [u8; 3]) {
        self.palette = (foreground, background);
        self.background = background;
    }

    /// Czech warning when the last scene did not fit into the memory of the graphics card (some of it is not drawn). Taken
    /// once.
    pub fn take_warning(&mut self) -> Option<String> {
        if let Some(w) = self.gpu_warning.take() {
            return Some(w);
        }
        let mut dynamic = false;
        for b in [&mut self.ov_lines, &mut self.ov_screen, &mut self.paper_lines, &mut self.paper_screen] {
            dynamic |= b.take_failure();
        }
        dynamic.then(|| gpubuf::GPU_DYNAMIC_WARNING.to_string())
    }

    /// Cuts the next scenes into buffers of at most `bytes` (never more than the device allows): tests of the splitting.
    pub fn set_buffer_chunk_bytes(&mut self, bytes: Option<u64>) {
        self.chunk_bytes_limit = bytes;
    }

    /// Behave as if the device refused buffers over `limit` bytes: the warning and the partial picture of a scene that does
    /// not fit. For tests, also those of the application.
    pub fn set_simulated_buffer_limit(&mut self, limit: Option<u64>) {
        self.simulated_buffer_limit = limit;
        for b in [&mut self.ov_lines, &mut self.ov_screen, &mut self.paper_lines, &mut self.paper_screen] {
            b.set_limit(limit);
        }
    }

    /// Number of buffers (fill corners, segments) the current scene is held in.
    pub fn buffer_counts(&self) -> (usize, usize) {
        (self.fills.bufs.len(), self.lines.bufs.len())
    }

    fn upload_scene(&mut self, scene: &Scene2d) {
        self.marks = None;
        self.upload_scene_from(scene, scene.batches.iter().map(Cow::Borrowed), &[]);
    }

    /// Uploads `batches` (the ones of `scene`, borrowed or owned); layers, bounds, pictures and the origin come from `scene`.
    fn upload_scene_from<'a>(&mut self, scene: &Scene2d, batches: impl Iterator<Item = Cow<'a, DrawBatch>>, roles: &[u8]) {
        let marks = self.marks;
        // coordinates of the scene are relative to its origin already; the bounds are world coordinates
        self.origin = Some(scene.origin).filter(|o| o[0].is_finite() && o[1].is_finite()).unwrap_or([0.0, 0.0]);
        let origin = self.origin;
        // buffers of at most `chunk` bytes: the largest buffer the device takes is 256 MiB, and 24 million fill corners
        // (10 000 texts) are 289 MB. A batch may span several buffers; it is drawn as several draw calls in order.
        let chunk = self.chunk_bytes_limit.unwrap_or(u64::MAX).min(gpubuf::chunk_bytes(&self.device));
        let line_cap = gpubuf::items_per_buffer(chunk, std::mem::size_of::<LineInst2d>(), 1);
        let fill_cap = gpubuf::items_per_buffer(chunk, std::mem::size_of::<FillVertex2d>(), 3);
        let mut lines = ChunkWriter::<LineInst2d>::new(&self.device, "2d lines", wgpu::BufferUsages::VERTEX, line_cap)
            .with_limit(self.simulated_buffer_limit)
            .with_queue(self.queue.as_ref());
        let mut fills = ChunkWriter::<FillVertex2d>::new(&self.device, "2d fills", wgpu::BufferUsages::VERTEX, fill_cap)
            .with_limit(self.simulated_buffer_limit)
            .with_queue(self.queue.as_ref());
        self.batches.clear();
        for (k, b) in batches.enumerate() {
            let color = match (marks, roles.get(k).copied().unwrap_or(0)) {
                (Some((fg, _)), 1) => pack_rgba(fg, b.color[3]),
                (Some((_, bg)), 2) => pack_rgba(bg, b.color[3]),
                _ => pack_rgba([b.color[0], b.color[1], b.color[2]], b.color[3]),
            };
            let l0 = lines.len();
            let f0 = fills.len();
            let mut bbox: Option<Rect2> = None;
            let mut grow = |p: [f32; 2]| {
                if !(p[0].is_finite() && p[1].is_finite()) {
                    return;
                }
                let (x, y) = (f64::from(p[0]) + origin[0], f64::from(p[1]) + origin[1]);
                bbox = Some(match bbox {
                    None => ([x, y], [x, y]),
                    Some((lo, hi)) => ([lo[0].min(x), lo[1].min(y)], [hi[0].max(x), hi[1].max(y)]),
                });
            };
            for seg in b.lines.chunks_exact(2) {
                grow(seg[0]);
                grow(seg[1]);
                lines.push(LineInst2d { p0: seg[0], p1: seg[1], width: b.width.max(0.0), color });
            }
            for tri in b.fills.chunks_exact(3) {
                for p in tri {
                    grow(*p);
                    fills.push(FillVertex2d { pos: *p, color });
                }
            }
            self.batches.push(Batch2d {
                layer: b.layer,
                lines: (l0 as u32, (lines.len() - l0) as u32),
                fills: (f0 as u32, (fills.len() - f0) as u32),
                bbox,
            });
        }
        let (lines, lines_failed, _) = lines.finish();
        let (fills, fills_failed, _) = fills.finish();
        self.gpu_warning = (lines_failed + fills_failed > 0).then(|| gpubuf::GPU_BUFFER_WARNING.to_string());
        self.lines = lines;
        self.fills = fills;
        self.layer_visible = scene.layers.iter().map(|l| (l.id, l.visible)).collect();
        self.bounds = scene.bounds.map(|(a, b)| (a, b));
        self.scene_pictures = scene
            .images
            .iter()
            .map(|im| ScenePicture {
                layer: im.layer,
                after: im.after,
                width: im.width,
                height: im.height,
                rgba: im.rgba.clone(),
                corners: im.corners,
            })
            .collect();
        self.gpu_pictures_ready = false;
        self.overlays.clear();
        // a new scene has its own batches: the highlight and the mask belonged to the old one
        self.hl_lines = Chunked::default();
        self.batch_mask = None;
    }

    // ------------------------------------------------------------------ highlight and batch mask

    /// Draws the highlight of the picked entity on top of the drawing. `batches` are what the scene builder gives for it:
    /// segments relative to the origin of the scene (the one of the last `set_scene`), `width` the pen in pixels (at least
    /// 2). They are made into buffers once, here, cut like the scene is (a block of 10 000 texts has millions of
    /// segments), and drawn from them in every frame: nothing is converted or uploaded per frame. A buffer that cannot be
    /// made leaves out the part of the highlight that was in it (`take_warning`).
    pub fn set_highlight(&mut self, batches: &[DrawBatch]) {
        let chunk = self.chunk_bytes_limit.unwrap_or(u64::MAX).min(gpubuf::chunk_bytes(&self.device));
        let cap = gpubuf::items_per_buffer(chunk, std::mem::size_of::<LineInst2d>(), 1);
        let mut w = ChunkWriter::<LineInst2d>::new(&self.device, "2d highlight", wgpu::BufferUsages::VERTEX, cap).with_limit(self.simulated_buffer_limit);
        for b in batches {
            let color = pack_rgba([b.color[0], b.color[1], b.color[2]], b.color[3]);
            let width = b.width.max(crate::overlay2d::SELECTION_MIN_PX);
            for seg in b.lines.chunks_exact(2) {
                w.push(LineInst2d { p0: seg[0], p1: seg[1], width, color });
            }
        }
        let (lines, failed, _) = w.finish();
        self.hl_lines = lines;
        if failed > 0 {
            self.gpu_warning = Some(gpubuf::GPU_DYNAMIC_WARNING.to_string());
        }
    }

    pub fn clear_highlight(&mut self) {
        self.hl_lines = Chunked::default();
    }

    /// Segments of the highlight that are on the graphics card, and the buffers they are in.
    pub fn highlight_counts(&self) -> (usize, usize) {
        (self.hl_lines.present_len(), self.hl_lines.bufs.iter().filter(|b| b.is_some()).count())
    }

    /// The palette part of the uniform block of the scene: markers (when the scene has batches of those roles) and the colours
    /// they are drawn in.
    fn palette_globals(&self) -> Globals2d {
        let f = |c: [u8; 3]| crate::colors::to_f32(c);
        let rgba = |c: [u8; 3], w: f32| {
            let v = f(c);
            [v[0], v[1], v[2], w]
        };
        let (fg, bg) = self.palette;
        let ((fm, bm), on) = match self.marks {
            Some(m) => (m, 1.0),
            None => (([0, 0, 0], [0, 0, 0]), 0.0),
        };
        Globals2d { fg_mark: rgba(fm, on), fg: rgba(fg, 1.0), bg_mark: rgba(bm, on), bg: rgba(bg, 1.0), ..Default::default() }
    }

    /// Bytes the overlay and paper buffers hold for the frame that was drawn last (tests: a highlight adds none).
    #[cfg(test)]
    pub(crate) fn overlay_bytes(&self) -> u64 {
        self.ov_lines.len + self.ov_screen.len + self.paper_lines.len + self.paper_screen.len
    }

    /// Which batches of the scene are drawn, one entry per batch of the last `set_scene` (a batch that depends on several
    /// layers is drawn only when all of them are shown). A layer that is switched on or off is a new mask, not a new
    /// scene. A mask of the wrong length is ignored.
    pub fn set_batch_mask(&mut self, mask: &[bool]) {
        self.batch_mask = (mask.len() == self.batches.len()).then(|| mask.to_vec());
    }

    fn batch_shown(&self, k: usize, b: &Batch2d) -> bool {
        self.layer_visible(b.layer) && self.batch_mask.as_ref().is_none_or(|m| m.get(k).copied().unwrap_or(true))
    }

    /// Uploads the pictures of the scene once a queue is at hand. Pictures whose side exceeds the device's texture size
    /// are left out.
    fn ensure_pictures(&mut self, queue: &wgpu::Queue) {
        if self.gpu_pictures_ready {
            return;
        }
        self.gpu_pictures_ready = true;
        self.gpu_pictures.clear();
        let max_side = self.device.limits().max_texture_dimension_2d;
        let mut verts: Vec<PictureVertex> = Vec::new();
        for pic in &self.scene_pictures {
            let (w, h) = (pic.width, pic.height);
            if w == 0 || h == 0 || w > max_side || h > max_side || pic.rgba.len() as u64 != u64::from(w) * u64::from(h) * 4 {
                continue;
            }
            let size = wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 };
            // a picture the graphics memory cannot take is left out with a warning, not a panic
            let made = gpubuf::guarded(&self.device, "picture 2d", || {
                self.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("picture 2d"),
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                })
            });
            let Ok(texture) = made else {
                self.gpu_warning = Some(gpubuf::GPU_BUFFER_WARNING.to_string());
                continue;
            };
            queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                &pic.rgba,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) },
                size,
            );
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("picture 2d"),
                layout: &self.layouts.picture,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.picture_sampler) },
                ],
            });
            // texel corners: top left, top right, bottom right, bottom left; two triangles
            let uv = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
            let first = verts.len() as u32;
            for i in [0usize, 3, 2, 0, 2, 1] {
                verts.push(PictureVertex { pos: pic.corners[i], uv: uv[i] });
            }
            self.gpu_pictures.push(GpuPicture { layer: pic.layer, after: pic.after, first, bind });
        }
        self.picture_buf = if verts.is_empty() {
            None
        } else {
            match gpubuf::try_buffer(&self.device, "2d pictures", bytemuck::cast_slice(&verts), wgpu::BufferUsages::VERTEX) {
                Ok(b) => Some(b),
                Err(_) => {
                    self.gpu_warning = Some(gpubuf::GPU_BUFFER_WARNING.to_string());
                    None
                }
            }
        };
    }

    /// Whether a layer is drawn (layers not in the scene default to visible).
    pub fn layer_visible(&self, layer: LayerId) -> bool {
        self.layer_visible.get(&layer).copied().unwrap_or(true)
    }

    /// Shows or hides one layer without drawing the scene again; pan and zoom stay.
    pub fn set_layer_visible(&mut self, layer: LayerId, visible: bool) {
        self.layer_visible.insert(layer, visible);
    }

    /// Only the given layers are visible.
    pub fn set_visible_layers(&mut self, visible: &HashSet<LayerId>) {
        for (id, v) in self.layer_visible.iter_mut() {
            *v = visible.contains(id);
        }
    }

    /// Bounds of what is shown: the batches of visible layers, else the scene bounds.
    pub fn visible_bounds(&self) -> Option<Rect2> {
        let mut out: Option<Rect2> = None;
        for (k, b) in self.batches.iter().enumerate() {
            if !self.batch_shown(k, b) {
                continue;
            }
            if let Some((lo, hi)) = b.bbox {
                out = Some(match out {
                    None => (lo, hi),
                    Some((a, c)) => ([a[0].min(lo[0]), a[1].min(lo[1])], [c[0].max(hi[0]), c[1].max(hi[1])]),
                });
            }
        }
        out.or(self.bounds)
    }

    /// Fits the visible content with the 3 % margin (`fit`).
    pub fn fit(&mut self) {
        if let Some(b) = self.visible_bounds() {
            self.view.fit(&b, self.viewport);
        }
    }

    /// Zooms to a rectangle with 20 % margin (`zoom_to_items`).
    pub fn zoom_to_rect(&mut self, r: &Rect2) {
        self.view.zoom_to_rect(r, self.viewport);
    }

    pub fn set_overlays(&mut self, overlays: Overlays2d) {
        self.overlays = overlays;
    }

    pub fn overlays(&self) -> &Overlays2d {
        &self.overlays
    }

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

    // ------------------------------------------------------------------ drawing

    fn build_ui(&mut self, view: &View2d, vp: Viewport, dpr: f64) -> UiLists {
        let mut out = UiLists::default();
        let d = dpr as f32;
        let px = |p: [f64; 2]| -> [f32; 2] {
            let s = view.to_screen(p, vp);
            [s[0] as f32, s[1] as f32]
        };
        let atlas = self.atlas.as_mut();
        let white = atlas.as_ref().map(|a| a.white_uv()).unwrap_or([0.0, 0.0]);
        let tri = |out: &mut UiLists, pts: [[f32; 2]; 3], color: u32| {
            for p in pts {
                out.polys.push(ScreenVertex { pos: p, uv: white, color });
            }
        };
        for t in &self.overlays.triangles {
            let col = pack_rgba(t.color, (t.alpha.clamp(0.0, 1.0) * 255.0).round() as u8);
            for tr in t.triangles.chunks_exact(3) {
                tri(&mut out, [px(tr[0]), px(tr[1]), px(tr[2])], col);
            }
        }
        for dot in &self.overlays.dots {
            let c = px(dot.pos);
            let r = dot.diameter_px * d / 2.0;
            let col = pack_rgba(dot.color, 255);
            const N: usize = 20;
            for i in 0..N {
                let (a0, a1) = (i as f32 / N as f32 * std::f32::consts::TAU, (i + 1) as f32 / N as f32 * std::f32::consts::TAU);
                tri(&mut out, [c, [c[0] + r * a0.cos(), c[1] + r * a0.sin()], [c[0] + r * a1.cos(), c[1] + r * a1.sin()]], col);
            }
        }
        let line = |out: &mut UiLists, a: [f32; 2], b: [f32; 2], width: f32, color: Rgb8, dashed: bool| {
            out.lines.push(LineInst {
                p0: [a[0], a[1], 0.0],
                width: width * d,
                p1: [b[0], b[1], 0.0],
                flags: LINE_PIXEL_SPACE,
                color: pack_rgba(color, 255),
                dash_on: if dashed { 4.0 * width * d } else { 0.0 },
                dash_off: if dashed { 2.0 * width * d } else { 0.0 },
                pad: 0.0,
            });
        };
        for l in &self.overlays.lines {
            for w in l.points.windows(2) {
                line(&mut out, px(w[0]), px(w[1]), l.width_px, l.color, l.dashed);
            }
        }
        for s in &self.overlays.squares {
            let c = px(s.pos);
            let h = s.size_px * d / 2.0;
            let corners = [[c[0] - h, c[1] - h], [c[0] + h, c[1] - h], [c[0] + h, c[1] + h], [c[0] - h, c[1] + h]];
            for i in 0..4 {
                line(&mut out, corners[i], corners[(i + 1) % 4], s.width_px, s.color, false);
            }
        }
        if let Some(atlas) = atlas {
            for t in &self.overlays.texts {
                push_text(
                    &mut out.text,
                    atlas,
                    &t.text,
                    px(t.pos),
                    t.size_px * d,
                    pack_rgba(t.color, 255),
                    t.h,
                    t.v,
                    TextFrame::default(),
                );
            }
        }
        out
    }

    fn upload_atlas(&mut self, queue: &wgpu::Queue) {
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

    fn render_into(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &Target2d,
        view: &View2d,
        vp: Viewport,
        dpr: f64,
        opts: &Opts2d,
    ) {
        let ui = if opts.ui { self.build_ui(view, vp, dpr) } else { UiLists::default() };
        // paper of thumbnails
        let mut paper = UiLists::default();
        if let (Some(r), Some(atlas)) = (opts.paper, self.atlas.as_ref()) {
            let white = pack_rgba([255, 255, 255], 255);
            let q = [[r[0] as f32, r[1] as f32], [r[2] as f32, r[1] as f32], [r[2] as f32, r[3] as f32], [r[0] as f32, r[3] as f32]];
            push_polygon(&mut paper.polys, atlas, &q, white);
            let (x0, y0, x1, y1) = (r[0] as f32 + 0.5, r[1] as f32 + 0.5, r[2] as f32 - 0.5, r[3] as f32 - 0.5);
            let c = [[x0, y0], [x1, y0], [x1, y1], [x0, y1]];
            for i in 0..4 {
                paper.lines.push(LineInst {
                    p0: [c[i][0], c[i][1], 0.0],
                    width: 1.0,
                    p1: [c[(i + 1) % 4][0], c[(i + 1) % 4][1], 0.0],
                    flags: LINE_PIXEL_SPACE,
                    color: pack_rgba(PAPER_BORDER_2D, 255),
                    dash_on: 0.0,
                    dash_off: 0.0,
                    pad: 0.0,
                });
            }
        }
        self.upload_atlas(queue);
        self.ensure_pictures(queue);

        let g2 = Globals2d {
            center: [(view.center[0] - self.origin[0]) as f32, (view.center[1] - self.origin[1]) as f32],
            viewport: [vp.width as f32, vp.height as f32],
            scale: view.scale as f32,
            width_mode: u32::from(self.width_mode == LineWidthMode::ScreenPixels),
            min_width: self.min_width_px * dpr as f32,
            px_scale: 1.0,
            ..self.palette_globals()
        };
        queue.write_buffer(&self.g2_buf, 0, bytemuck::bytes_of(&g2));
        // the highlight is drawn with pens of a width in pixels that follows the device pixel ratio, as the overlays are
        queue.write_buffer(
            &self.hl_g2_buf,
            0,
            bytemuck::bytes_of(&Globals2d { width_mode: 2, px_scale: dpr as f32, fg_mark: [0.0; 4], bg_mark: [0.0; 4], ..g2 }),
        );
        let mut ug = Globals::default();
        ug.crop = crate::gpu_types::mat4_to_array(&glam::DMat4::IDENTITY);
        ug.viewport = [vp.width as f32, vp.height as f32, dpr as f32, 0.0];
        queue.write_buffer(&self.ui_buf, 0, bytemuck::bytes_of(&ug));

        self.ov_lines.upload(&self.device, queue, bytemuck::cast_slice(&ui.lines));
        let n_ui_poly = ui.polys.len() as u32;
        let mut screen = ui.polys;
        screen.extend_from_slice(&ui.text);
        let n_ui_screen = screen.len() as u32;
        self.ov_screen.upload(&self.device, queue, bytemuck::cast_slice(&screen));
        self.paper_lines.upload(&self.device, queue, bytemuck::cast_slice(&paper.lines));
        self.paper_screen.upload(&self.device, queue, bytemuck::cast_slice(&paper.polys));
        let n_ui_lines = ui.lines.len() as u32;
        let n_paper_lines = paper.lines.len() as u32;
        let n_paper_polys = paper.polys.len() as u32;

        let clear = match opts.background {
            Some(c) => {
                let f = crate::colors::to_f32(c);
                wgpu::Color { r: f64::from(f[0]), g: f64::from(f[1]), b: f64::from(f[2]), a: 1.0 }
            }
            None => wgpu::Color::TRANSPARENT,
        };
        let (view_tex, resolve) = match &target.msaa_view {
            Some(m) => (m, Some(&target.color_view)),
            None => (&target.color_view, None),
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("drawing"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: view_tex,
                depth_slice: None,
                resolve_target: resolve,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(clear),
                    store: if resolve.is_some() { wgpu::StoreOp::Discard } else { wgpu::StoreOp::Store },
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        // paper
        if n_paper_polys > 0 {
            if let Some(b) = &self.paper_screen.buf {
                pass.set_pipeline(&self.ui_pipes.screen);
                pass.set_bind_group(0, &self.ui_bind, &[]);
                pass.set_bind_group(1, &self.atlas_bind, &[]);
                pass.set_vertex_buffer(0, b.slice(..));
                pass.draw(0..n_paper_polys, 0..1);
            }
        }
        // drawing in z-order: a picture is drawn before the batch index it was recorded at (`DrawImage::after`), then
        // each batch as fills, then lines
        let mut next_picture = 0usize;
        for k in 0..=self.batches.len() {
            while next_picture < self.gpu_pictures.len() && self.gpu_pictures[next_picture].after as usize <= k {
                let pic = &self.gpu_pictures[next_picture];
                next_picture += 1;
                if !self.layer_visible(pic.layer) {
                    continue;
                }
                if let Some(pb) = &self.picture_buf {
                    pass.set_pipeline(&self.scene_pipes.pictures);
                    pass.set_bind_group(0, &self.g2_bind, &[]);
                    pass.set_bind_group(1, &pic.bind, &[]);
                    pass.set_vertex_buffer(0, pb.slice(..));
                    pass.draw(pic.first..pic.first + 6, 0..1);
                }
            }
            let Some(b) = self.batches.get(k) else { break };
            if !self.batch_shown(k, b) {
                continue;
            }
            // a batch is one range of the whole sequence of corners / segments; it is drawn buffer by buffer, in order
            if b.fills.1 > 0 {
                pass.set_pipeline(&self.scene_pipes.fills);
                pass.set_bind_group(0, &self.g2_bind, &[]);
                for (buf, first, n) in self.fills.pieces(b.fills.0 as usize, b.fills.1 as usize) {
                    pass.set_vertex_buffer(0, buf.slice(..));
                    pass.draw(first as u32..(first + n) as u32, 0..1);
                }
            }
            if b.lines.1 > 0 {
                pass.set_pipeline(&self.scene_pipes.lines);
                pass.set_bind_group(0, &self.g2_bind, &[]);
                for (buf, first, n) in self.lines.pieces(b.lines.0 as usize, b.lines.1 as usize) {
                    pass.set_vertex_buffer(0, buf.slice(..));
                    pass.draw(0..6, first as u32..(first + n) as u32);
                }
            }
        }
        // highlight of the picked entity: above the drawing, below the measurement and the snap marker
        if opts.ui && !self.hl_lines.is_empty() {
            pass.set_pipeline(&self.scene_pipes.lines);
            pass.set_bind_group(0, &self.hl_g2_bind, &[]);
            for (buf, first, n) in self.hl_lines.pieces(0, self.hl_lines.len) {
                pass.set_vertex_buffer(0, buf.slice(..));
                pass.draw(0..6, first as u32..(first + n) as u32);
            }
        }
        // paper border and overlays
        pass.set_bind_group(0, &self.ui_bind, &[]);
        if n_paper_lines > 0 {
            if let Some(b) = &self.paper_lines.buf {
                pass.set_pipeline(&self.ui_pipes.lines);
                pass.set_vertex_buffer(0, b.slice(..));
                pass.draw(0..6, 0..n_paper_lines);
            }
        }
        if n_ui_poly > 0 {
            if let Some(b) = &self.ov_screen.buf {
                pass.set_pipeline(&self.ui_pipes.screen);
                pass.set_bind_group(1, &self.atlas_bind, &[]);
                pass.set_vertex_buffer(0, b.slice(..));
                pass.draw(0..n_ui_poly, 0..1);
            }
        }
        if n_ui_lines > 0 {
            if let Some(b) = &self.ov_lines.buf {
                pass.set_pipeline(&self.ui_pipes.lines);
                pass.set_vertex_buffer(0, b.slice(..));
                pass.draw(0..6, 0..n_ui_lines);
            }
        }
        if n_ui_screen > n_ui_poly {
            if let Some(b) = &self.ov_screen.buf {
                pass.set_pipeline(&self.ui_pipes.screen);
                pass.set_bind_group(1, &self.atlas_bind, &[]);
                pass.set_vertex_buffer(0, b.slice(..));
                pass.draw(n_ui_poly..n_ui_screen, 0..1);
            }
        }
    }

    pub(crate) fn prepare_frame(&mut self, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, width: u32, height: u32, dpr: f64) {
        if width == 0 || height == 0 {
            return;
        }
        self.queue = Some(queue.clone());
        self.set_viewport(width, height, dpr);
        let stale = match &self.target {
            Some(t) => t.width != width || t.height != height || t.samples != self.samples,
            None => true,
        };
        if stale {
            let t = Target2d::new(&self.device, width, height, self.samples);
            self.blit_bind = Some(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("blit 2d"),
                layout: &self.layouts.blit,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&t.color_view) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.blit_sampler) },
                ],
            }));
            self.target = Some(t);
        }
        let Some(target) = self.target.take() else { return };
        let (view, vp) = (self.view, self.viewport);
        let opts = Opts2d { background: Some(self.background), paper: None, ui: true };
        self.render_into(queue, encoder, &target, &view, vp, dpr, &opts);
        self.target = Some(target);
    }

    pub(crate) fn blit(&self, pass: &mut wgpu::RenderPass<'static>) {
        if let Some(bind) = &self.blit_bind {
            pass.set_pipeline(&self.blit_pipe);
            pass.set_bind_group(0, bind, &[]);
            pass.draw(0..3, 0..1);
        }
    }

    /// Paint callback for the rectangle `rect` (egui points).
    pub fn paint_callback(this: &Arc<Mutex<Renderer2d>>, rect: egui::Rect) -> egui::PaintCallback {
        egui_wgpu::Callback::new_paint_callback(rect, Renderer2dCallback { renderer: Arc::clone(this), rect })
    }

    // ------------------------------------------------------------------ captures

    fn capture(&mut self, view: &View2d, w: u32, h: u32, dpr: f64, opts: &Opts2d) -> Result<Rgba8, RenderError> {
        let queue = self.queue.clone().ok_or(RenderError::NoQueue)?;
        if w == 0 || h == 0 || w > crate::capture::MAX_CAPTURE_SIDE || h > crate::capture::MAX_CAPTURE_SIDE {
            return Err(RenderError::BadSize(w, h));
        }
        let target = Target2d::new(&self.device, w, h, self.samples);
        let device = self.device.clone();
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("capture 2d") });
        self.render_into(&queue, &mut encoder, &target, view, Viewport::new(f64::from(w), f64::from(h)), dpr, opts);
        let bpr = (w * 4).div_ceil(256) * 256;
        let buffer = crate::gpubuf::guarded(&device, "capture 2d readback", || {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("capture 2d readback"),
                size: u64::from(bpr) * u64::from(h),
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            })
        })
        .map_err(RenderError::ReadBack)?;
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &target.color,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bpr), rows_per_image: Some(h) },
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        queue.submit(Some(encoder.finish()));
        let raw = read_buffer(&device, &buffer)?;
        let mut data = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            let o = (y * bpr) as usize;
            data.extend_from_slice(&raw[o..o + (w * 4) as usize]);
        }
        unpremultiply(&mut data);
        Ok(Rgba8 { width: w, height: h, data })
    }

    /// Picture of the drawing as shown (current pan, zoom, layers, overlays) in a picture of the given size; the view
    /// is the renderer's, scaled so that the same region is visible.
    pub fn capture_rgba(&mut self, width: u32, height: u32, transparent_background: bool) -> Result<Rgba8, RenderError> {
        let opts = Opts2d { background: (!transparent_background).then_some(self.background), paper: None, ui: true };
        let view = self.view;
        self.capture(&view, width, height, 1.0, &opts)
    }

    /// `capture_rgba` for a screen with the device pixel ratio `dpr` (pens of the interface and the highlight are that many
    /// times wider; drawing pens and the view are not).
    pub fn capture_rgba_dpr(&mut self, width: u32, height: u32, dpr: f64) -> Result<Rgba8, RenderError> {
        let opts = Opts2d { background: Some(self.background), paper: None, ui: true };
        let view = self.view;
        self.capture(&view, width, height, dpr.max(0.1), &opts)
    }

    /// PNG of [`Renderer2d::capture_rgba`].
    pub fn capture_png(&mut self, width: u32, height: u32, transparent_background: bool) -> Result<Vec<u8>, RenderError> {
        let img = self.capture_rgba(width, height, transparent_background)?;
        png_from_rgba(img.width, img.height, img.data)
    }

    /// Explorer thumbnail (`render_thumbnail`): the visible content on white paper with a 3 % margin and a grey
    /// border, centred in a transparent square.
    pub fn thumbnail_rgba(&mut self, size: u32) -> Result<Rgba8, RenderError> {
        let size = clamp_thumbnail_size(size);
        let Some((lo, hi)) = self.visible_bounds() else { return Err(RenderError::Empty) };
        let (w, h) = (hi[0] - lo[0], hi[1] - lo[1]);
        if !(w >= 0.0 && h >= 0.0) || (w <= 0.0 && h <= 0.0) {
            return Err(RenderError::Empty);
        }
        let m = w.max(h) * 0.03;
        let (rw, rh) = (w + 2.0 * m, h + 2.0 * m);
        let s = (f64::from(size) / rw).min(f64::from(size) / rh);
        let (tw, th) = (rw * s, rh * s);
        let (x0, y0) = ((f64::from(size) - tw) / 2.0, (f64::from(size) - th) / 2.0);
        let view = View2d { center: [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0], scale: s };
        let opts = Opts2d { background: None, paper: Some([x0, y0, x0 + tw, y0 + th]), ui: false };
        self.capture(&view, size, size, 1.0, &opts)
    }

    pub fn thumbnail_png(&mut self, size: u32) -> Result<Vec<u8>, RenderError> {
        let img = self.thumbnail_rgba(size)?;
        png_from_rgba(img.width, img.height, img.data)
    }
}

/// egui_wgpu callback of the 2D view.
pub struct Renderer2dCallback {
    renderer: Arc<Mutex<Renderer2d>>,
    rect: egui::Rect,
}

impl egui_wgpu::CallbackTrait for Renderer2dCallback {
    fn prepare(
        &self,
        _device: &wgpu::Device,
        queue: &wgpu::Queue,
        screen: &egui_wgpu::ScreenDescriptor,
        encoder: &mut wgpu::CommandEncoder,
        _resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Ok(mut r) = self.renderer.lock() else { return Vec::new() };
        let vp = egui::epaint::ViewportInPixels::from_points(&self.rect, screen.pixels_per_point, screen.size_in_pixels);
        r.prepare_frame(queue, encoder, vp.width_px.max(0) as u32, vp.height_px.max(0) as u32, f64::from(screen.pixels_per_point));
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
