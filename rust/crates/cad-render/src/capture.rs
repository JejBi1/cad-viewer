//! Offscreen pictures: screenshots (`view.Dump`), and the headless thumbnail renderer used by `--thumbnail`.
use std::io::Cursor;

use eframe::wgpu;
use image::{imageops, ImageFormat, RgbaImage};

use crate::camera::{Camera, StdView, Viewport};
use crate::colors::THUMB_EDGE_COLOR;
use crate::frame::RenderOpts;
use crate::gpu::RenderError;
use crate::pickpass::read_buffer;
use crate::pipelines::{Pipelines, COLOR_FORMAT};
use crate::renderer3d::{probe_sample_count, FrameTarget, Renderer3d};

/// Largest accepted picture side in pixels.
pub const MAX_CAPTURE_SIDE: u32 = 8192;

/// Thumbnail size clamp of `thumbs.run`: `max(16, min(size, 1024))`.
pub fn clamp_thumbnail_size(size: u32) -> u32 {
    size.clamp(16, 1024)
}

/// Render size of a thumbnail: `min(max(size, 64), 1024) * 2` (rendered twice as large, then scaled down).
pub fn thumbnail_render_size(size: u32) -> u32 {
    size.clamp(64, 1024) * 2
}

/// Margin of the thumbnail fit (`FitAll(0.01)`).
pub const THUMBNAIL_FIT_MARGIN: f64 = 0.01;
/// Alpha threshold of the content crop (`alpha > 0.02`) and the covered fraction of the canvas.
pub const THUMBNAIL_ALPHA_MIN: f32 = 0.02;
pub const THUMBNAIL_COVERAGE: f64 = 0.94;

#[derive(Clone, Debug)]
pub struct CaptureOptions {
    pub width: u32,
    pub height: u32,
    /// Leave the background transparent (thumbnails) instead of the gradient.
    pub transparent_background: bool,
    /// Draw view cube, scale bar, labels and overlays (screenshots of the window do, thumbnails do not).
    pub ui: bool,
    /// Device pixel ratio for overlay sizes.
    pub dpr: f64,
    /// Camera of the picture; `None` = the renderer's current camera.
    pub camera: Option<Camera>,
    /// MSAA samples; `None` = the renderer's.
    pub msaa: Option<u32>,
}

impl CaptureOptions {
    /// Screenshot of the view as shown: gradient background and overlays.
    pub fn screenshot(width: u32, height: u32) -> Self {
        Self { width, height, transparent_background: false, ui: true, dpr: 1.0, camera: None, msaa: None }
    }
}

/// Straight-alpha RGBA picture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rgba8 {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl Rgba8 {
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let o = ((y * self.width + x) * 4) as usize;
        [self.data[o], self.data[o + 1], self.data[o + 2], self.data[o + 3]]
    }

    pub fn to_png(&self) -> Result<Vec<u8>, RenderError> {
        png_from_rgba(self.width, self.height, self.data.clone())
    }
}

/// Encodes straight-alpha RGBA bytes as PNG.
pub fn png_from_rgba(width: u32, height: u32, data: Vec<u8>) -> Result<Vec<u8>, RenderError> {
    let img = RgbaImage::from_raw(width, height, data).ok_or(RenderError::BadSize(width, height))?;
    let mut out = Vec::new();
    img.write_to(&mut Cursor::new(&mut out), ImageFormat::Png).map_err(|e| RenderError::Png(e.to_string()))?;
    Ok(out)
}

/// Converts premultiplied RGBA to straight alpha in place.
pub fn unpremultiply(data: &mut [u8]) {
    for px in data.chunks_exact_mut(4) {
        let a = u32::from(px[3]);
        if a == 0 {
            px[0] = 0;
            px[1] = 0;
            px[2] = 0;
        } else if a < 255 {
            for c in &mut px[..3] {
                *c = ((u32::from(*c) * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
}

/// Premultiplies straight-alpha RGBA in place.
pub fn premultiply(data: &mut [u8]) {
    for px in data.chunks_exact_mut(4) {
        let a = u32::from(px[3]);
        for c in &mut px[..3] {
            *c = ((u32::from(*c) * a + 127) / 255) as u8;
        }
    }
}

/// `_matte`: crop to the content (alpha above 0.02), scale it into `0.94 * size` keeping the aspect ratio with a
/// smooth filter and centre it on a transparent `size x size` canvas. An empty picture comes back as it is, resized
/// to the canvas.
pub fn thumbnail_from(img: &Rgba8, size: u32) -> Rgba8 {
    let size = size.max(1);
    let min_a = (THUMBNAIL_ALPHA_MIN * 255.0).floor() as u8; // alpha > 0.02 means above 5.1
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0u32, 0u32);
    let mut any = false;
    for y in 0..img.height {
        for x in 0..img.width {
            if img.data[((y * img.width + x) * 4 + 3) as usize] > min_a {
                any = true;
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    let mut canvas = vec![0u8; (size * size * 4) as usize];
    if !any {
        return Rgba8 { width: size, height: size, data: canvas };
    }
    let (cw, ch) = (x1 - x0 + 1, y1 - y0 + 1);
    let mut cropped = Vec::with_capacity((cw * ch * 4) as usize);
    for y in y0..=y1 {
        let o = ((y * img.width + x0) * 4) as usize;
        cropped.extend_from_slice(&img.data[o..o + (cw * 4) as usize]);
    }
    premultiply(&mut cropped);
    let Some(src) = RgbaImage::from_raw(cw, ch, cropped) else {
        return Rgba8 { width: size, height: size, data: canvas };
    };
    let box_px = ((f64::from(size) * THUMBNAIL_COVERAGE) as u32).max(1);
    let k = (f64::from(box_px) / f64::from(cw)).min(f64::from(box_px) / f64::from(ch));
    let (nw, nh) = (((f64::from(cw) * k).round() as u32).clamp(1, size), ((f64::from(ch) * k).round() as u32).clamp(1, size));
    let mut scaled = imageops::resize(&src, nw, nh, imageops::FilterType::Lanczos3).into_raw();
    unpremultiply(&mut scaled);
    let (ox, oy) = ((size - nw) / 2, (size - nh) / 2);
    for y in 0..nh {
        let s = (y * nw * 4) as usize;
        let d = (((oy + y) * size + ox) * 4) as usize;
        canvas[d..d + (nw * 4) as usize].copy_from_slice(&scaled[s..s + (nw * 4) as usize]);
    }
    Rgba8 { width: size, height: size, data: canvas }
}

impl Renderer3d {
    /// Renders the current model into a straight-alpha RGBA picture of the requested size.
    pub fn capture_rgba(&mut self, opts: &CaptureOptions) -> Result<Rgba8, RenderError> {
        let queue = self.queue_or_err()?;
        let (w, h) = (opts.width, opts.height);
        if w == 0 || h == 0 || w > MAX_CAPTURE_SIDE || h > MAX_CAPTURE_SIDE {
            return Err(RenderError::BadSize(w, h));
        }
        let wanted = opts.msaa.unwrap_or(self.style.msaa);
        let samples = if wanted == self.style.msaa { self.samples } else { probe_sample_count(&self.device, wanted) };
        let swapped = (samples != self.samples)
            .then(|| std::mem::replace(&mut self.pipelines, Pipelines::new(&self.device, &self.layouts, samples)));
        let target = FrameTarget::new(&self.device, w, h, samples);
        let camera = opts.camera.unwrap_or(self.camera);
        let device = self.device.clone();
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("capture") });
        self.render_into(
            &queue,
            &mut encoder,
            &target,
            &camera,
            Viewport::new(f64::from(w), f64::from(h)),
            opts.dpr,
            &RenderOpts { transparent_background: opts.transparent_background, ui: opts.ui },
        );
        if let Some(old) = swapped {
            // pipelines are only needed until the commands are recorded
            self.pipelines = old;
        }
        let bpr = (w * 4).div_ceil(256) * 256;
        let buffer = crate::gpubuf::guarded(&device, "capture readback", || {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("capture readback"),
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
        debug_assert_eq!(COLOR_FORMAT, wgpu::TextureFormat::Rgba8Unorm);
        unpremultiply(&mut data);
        Ok(Rgba8 { width: w, height: h, data })
    }

    /// PNG of the view (`screenshot(path)`): see [`CaptureOptions`].
    pub fn capture_png(&mut self, opts: &CaptureOptions) -> Result<Vec<u8>, RenderError> {
        self.capture_rgba(opts)?.to_png()
    }

    /// Explorer thumbnail of the loaded model: isometric view, transparent background, thin dark edges, rendered at
    /// twice the size, cropped to the content and centred with 6 % margin (the OpenGL path of `thumbs.render_3d`).
    pub fn thumbnail_rgba(&mut self, size: u32) -> Result<Rgba8, RenderError> {
        let size = clamp_thumbnail_size(size);
        let rs = thumbnail_render_size(size);
        let mut cam = Camera::default();
        cam.set_view(StdView::Iso);
        if let Some(b) = self.model_bounds() {
            cam.fit_box(&b, THUMBNAIL_FIT_MARGIN, Viewport::new(f64::from(rs), f64::from(rs)));
        }
        let saved = (self.style.edge_color, self.style.edges, self.style.wireframe, self.section);
        self.style.edge_color = THUMB_EDGE_COLOR;
        self.style.edges = true;
        self.style.wireframe = false;
        self.section = None;
        let result = self.capture_rgba(&CaptureOptions {
            width: rs,
            height: rs,
            transparent_background: true,
            ui: false,
            dpr: 1.0,
            camera: Some(cam),
            msaa: Some(8),
        });
        (self.style.edge_color, self.style.edges, self.style.wireframe, self.section) = saved;
        Ok(thumbnail_from(&result?, size))
    }

    /// PNG bytes of [`Renderer3d::thumbnail_rgba`].
    pub fn thumbnail_png(&mut self, size: u32) -> Result<Vec<u8>, RenderError> {
        self.thumbnail_rgba(size)?.to_png()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, rect: (u32, u32, u32, u32), c: [u8; 4]) -> Rgba8 {
        let mut data = vec![0u8; (w * h * 4) as usize];
        for y in rect.1..rect.1 + rect.3 {
            for x in rect.0..rect.0 + rect.2 {
                let o = ((y * w + x) * 4) as usize;
                data[o..o + 4].copy_from_slice(&c);
            }
        }
        Rgba8 { width: w, height: h, data }
    }

    #[test]
    fn size_rules_of_the_thumbnail_renderer() {
        assert_eq!(clamp_thumbnail_size(4), 16);
        assert_eq!(clamp_thumbnail_size(256), 256);
        assert_eq!(clamp_thumbnail_size(5000), 1024);
        assert_eq!(thumbnail_render_size(16), 128);
        assert_eq!(thumbnail_render_size(256), 512);
        assert_eq!(thumbnail_render_size(2000), 2048);
    }

    #[test]
    fn premultiply_roundtrip() {
        let mut d = vec![200, 100, 50, 128, 10, 20, 30, 0, 255, 255, 255, 255];
        let orig = d.clone();
        unpremultiply(&mut d);
        // transparent pixel loses its colour, opaque stays
        assert_eq!(&d[4..8], &[0, 0, 0, 0]);
        assert_eq!(&d[8..12], &orig[8..12]);
        premultiply(&mut d);
        assert!((i32::from(d[0]) - 200).abs() <= 1 || d[0] == 128, "{}", d[0]);
        // half-transparent premultiplied (100, 50, 25, 128) -> straight (199, 100, 50)
        let mut p = vec![100, 50, 25, 128];
        unpremultiply(&mut p);
        assert_eq!(p, vec![199, 100, 50, 128]);
    }

    #[test]
    fn thumbnail_crops_scales_and_centres() {
        // a 40 x 20 content block somewhere in a 200 x 200 transparent picture
        let img = solid(200, 200, (130, 150, 40, 20), [255, 0, 0, 255]);
        let t = thumbnail_from(&img, 100);
        assert_eq!((t.width, t.height), (100, 100));
        // content fits in 94 px: 40 x 20 scaled by 94 / 40 = 2.35 -> 94 x 47
        let mut xs = (u32::MAX, 0u32);
        let mut ys = (u32::MAX, 0u32);
        for y in 0..100 {
            for x in 0..100 {
                if t.pixel(x, y)[3] > 128 {
                    xs = (xs.0.min(x), xs.1.max(x));
                    ys = (ys.0.min(y), ys.1.max(y));
                }
            }
        }
        assert!((xs.1 - xs.0 + 1).abs_diff(94) <= 2, "{xs:?}");
        assert!((ys.1 - ys.0 + 1).abs_diff(47) <= 2, "{ys:?}");
        // centred (integer offset (size - w) // 2)
        assert!(xs.0.abs_diff(3) <= 1 && ys.0.abs_diff(26) <= 2, "{xs:?} {ys:?}");
        // colour is kept at the centre
        assert_eq!(t.pixel(50, 50), [255, 0, 0, 255]);
        // the corner stays transparent
        assert_eq!(t.pixel(0, 0)[3], 0);
    }

    #[test]
    fn thumbnail_of_an_empty_picture_is_transparent() {
        let img = solid(64, 64, (0, 0, 0, 0), [0; 4]);
        let t = thumbnail_from(&img, 32);
        assert_eq!((t.width, t.height), (32, 32));
        assert!(t.data.iter().all(|b| *b == 0));
        // alpha at or below 0.02 is not content
        let faint = solid(64, 64, (10, 10, 20, 20), [10, 10, 10, 5]);
        assert!(thumbnail_from(&faint, 32).data.iter().all(|b| *b == 0));
    }

    #[test]
    fn png_encoding_roundtrips() {
        let img = solid(8, 4, (2, 1, 3, 2), [10, 20, 30, 255]);
        let png = img.to_png().expect("png");
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        let back = image::load_from_memory(&png).expect("decode").to_rgba8();
        assert_eq!(back.dimensions(), (8, 4));
        assert_eq!(back.get_pixel(2, 1).0, [10, 20, 30, 255]);
        assert!(png_from_rgba(3, 3, vec![0; 5]).is_err());
    }
}
