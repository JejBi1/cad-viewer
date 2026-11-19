//! Raster pictures of DXF IMAGE entities: local path lookup (ezdxf `_find_image_path`), bounded decoding of PNG, JPEG,
//! GIF, TIFF and BMP, and the pixel operations ezdxf's Qt backend applies before drawing (contrast, fade, brightness,
//! transparency and the clipping mask).
use crate::info;
use image::{ImageFormat, ImageReader, Limits};
use std::io::Cursor;
use std::path::{Path, PathBuf};

/// Largest file read for one picture (bytes).
pub const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
/// Largest picture decoded (pixels).
pub const MAX_PIXELS: u64 = 25_000_000;
/// Largest picture side the decoder accepts (pixels).
pub const MAX_SIDE: u32 = 20_000;
/// Memory the decoder may allocate for one picture (bytes).
pub const MAX_ALLOC: u64 = 256 * 1024 * 1024;
/// Pixels of all pictures placed in one scene (RGBA copies with their effects applied).
pub const MAX_SCENE_PIXELS: u64 = 40_000_000;

// Pictures decoded by the current thread (tests count them).
#[cfg(test)]
thread_local! {
    pub static DECODES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// A decoded picture: straight RGBA, 8 bits per channel, first row at the top.
#[derive(Clone, Debug)]
pub struct Raster {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Local file of an IMAGEDEF name, as ezdxf finds it next to the drawing (`ezdxf.addons.drawing.frontend._find_image_path`):
/// the name as an absolute path, then relative to the drawing folder, then the file name in the drawing folder.
/// Remote names (UNC, device paths, URLs; see `info::is_remote_path`) are refused before any file is looked at.
/// `is_file` is the only file system access; it is a parameter so that tests can count the probes.
/// Unlike ezdxf, a relative name is not looked up in the working directory of the process.
pub fn resolve_path(doc_dir: Option<&Path>, filename: &str, is_file: &mut dyn FnMut(&Path) -> bool) -> Option<PathBuf> {
    if info::is_remote_path(filename) || filename.trim().is_empty() {
        return None;
    }
    let dir = doc_dir?;
    let local = filename.replace('\\', std::path::MAIN_SEPARATOR_STR);
    let direct = Path::new(&local);
    if direct.is_absolute() && is_file(direct) {
        return Some(direct.to_path_buf());
    }
    let relative = dir.join(&local);
    if is_file(&relative) {
        return Some(relative);
    }
    let by_name = dir.join(Path::new(&local).file_name()?);
    if is_file(&by_name) {
        return Some(by_name);
    }
    None
}

/// Reads and decodes a local picture; `None` for a missing, too large, unsupported or damaged file.
pub fn load_file(path: &Path) -> Option<Raster> {
    load_file_within(path, MAX_PIXELS)
}

/// `load_file` for a picture of at most `max_pixels` pixels (the size is read from the header, before decoding).
pub fn load_file_within(path: &Path, max_pixels: u64) -> Option<Raster> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    decode_within(&bytes, max_pixels)
}

/// Decodes PNG, JPEG, GIF (first frame), TIFF (first page) or BMP, detected from the content. Other formats and
/// pictures above `MAX_PIXELS` give `None`.
pub fn decode(bytes: &[u8]) -> Option<Raster> {
    decode_within(bytes, MAX_PIXELS)
}

/// `decode` for pictures of at most `max_pixels` pixels (never more than `MAX_PIXELS`).
pub fn decode_within(bytes: &[u8], max_pixels: u64) -> Option<Raster> {
    let max_pixels = max_pixels.min(MAX_PIXELS);
    let probe = ImageReader::new(Cursor::new(bytes)).with_guessed_format().ok()?;
    let format = probe.format()?;
    if !matches!(format, ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::Gif | ImageFormat::Tiff | ImageFormat::Bmp) {
        return None;
    }
    let (w, h) = probe.into_dimensions().ok()?;
    if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE || u64::from(w) * u64::from(h) > max_pixels {
        return None;
    }
    #[cfg(test)]
    DECODES.with(|c| c.set(c.get() + 1));
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(MAX_ALLOC);
    reader.limits(limits);
    let img = reader.decode().ok()?;
    let rgba = img.into_rgba8();
    if rgba.dimensions() != (w, h) {
        return None;
    }
    Some(Raster { width: w, height: h, rgba: rgba.into_raw() })
}

/// Blend of two 8 bit values: `a + t (b - a)`, clamped and truncated (PIL `Image.blend`).
fn blend(a: u8, b: u8, t: f64) -> u8 {
    (f64::from(a) + t * (f64::from(b) - f64::from(a))).clamp(0.0, 255.0) as u8
}

/// Brightness, contrast and fade as ezdxf's Qt backend applies them, in that order. Values are the DXF codes 281, 282
/// and 283 (defaults 50, 50, 0). `bg` is the background colour the fade blends towards.
pub fn adjust_colors(rgba: &mut [u8], brightness: i32, contrast: i32, fade: i32, bg: [u8; 3]) {
    if contrast != 50 {
        // PIL ImageEnhance.Contrast: blend with a flat grey of the mean luminance (ITU-R 601-2, PIL integer luma)
        let n = rgba.len() / 4;
        if n > 0 {
            let sum: u64 = rgba
                .chunks_exact(4)
                .map(|p| (u64::from(p[0]) * 19595 + u64::from(p[1]) * 38470 + u64::from(p[2]) * 7471 + 0x8000) >> 16)
                .sum();
            let mean = ((sum as f64 / n as f64) + 0.5).floor().clamp(0.0, 255.0) as u8;
            let factor = f64::from(contrast) / 50.0;
            for p in rgba.chunks_exact_mut(4) {
                for c in &mut p[..3] {
                    *c = blend(mean, *c, factor);
                }
            }
        }
    }
    if fade != 0 {
        let amount = f64::from(fade) / 100.0;
        for p in rgba.chunks_exact_mut(4) {
            for (c, b) in p[..3].iter_mut().zip(bg) {
                *c = blend(*c, b, amount);
            }
        }
    }
    if brightness != 50 {
        let amount = f64::from(brightness) / 50.0 - 1.0;
        let (target, t) = if amount > 0.0 { (255u8, amount) } else { (0u8, -amount) };
        for p in rgba.chunks_exact_mut(4) {
            for c in &mut p[..3] {
                *c = blend(*c, target, t);
            }
        }
    }
}

/// Alpha of a picture: 255 everywhere unless the entity uses transparency (DXF flag 8), then multiplied by
/// `alpha_factor` (1 - transparency; the result is truncated like ezdxf's `astype(uint8)`).
pub fn apply_alpha(rgba: &mut [u8], use_transparency: bool, alpha_factor: f64) {
    for p in rgba.chunks_exact_mut(4) {
        if !use_transparency {
            p[3] = 255;
        }
        p[3] = (f64::from(p[3]) * alpha_factor).clamp(0.0, 255.0) as u8;
    }
}

/// Pixels inside any of the polygons (union of the even-odd fills), as 1 = inside, row major. Polygons are in pixel
/// coordinates of the picture, where pixel (i, j) is tested at the point (i, j): the same convention as PIL's
/// `ImageDraw.polygon`, which ezdxf uses for the clipping mask.
pub fn polygons_mask(w: u32, h: u32, polys: &[Vec<[f64; 2]>]) -> Vec<u8> {
    let (wu, hu) = (w as usize, h as usize);
    let mut mask = vec![0u8; wu * hu];
    let mut xs: Vec<f64> = Vec::new();
    for poly in polys {
        let n = poly.len();
        if n < 3 || poly.iter().any(|p| !p[0].is_finite() || !p[1].is_finite()) {
            continue;
        }
        let ymin = poly.iter().map(|p| p[1]).fold(f64::INFINITY, f64::min);
        let ymax = poly.iter().map(|p| p[1]).fold(f64::NEG_INFINITY, f64::max);
        let j0 = ymin.ceil().max(0.0);
        let j1 = ymax.floor().min(f64::from(h) - 1.0);
        if j1 < j0 {
            continue;
        }
        for j in j0 as usize..=j1 as usize {
            let y = j as f64;
            xs.clear();
            for k in 0..n {
                let (a, b) = (poly[k], poly[(k + 1) % n]);
                if (a[1] <= y && y < b[1]) || (b[1] <= y && y < a[1]) {
                    xs.push(a[0] + (y - a[1]) * (b[0] - a[0]) / (b[1] - a[1]));
                }
            }
            xs.sort_by(|p, q| p.partial_cmp(q).unwrap_or(std::cmp::Ordering::Equal));
            for pair in xs.chunks_exact(2) {
                let x0 = pair[0].ceil().max(0.0);
                let x1 = pair[1].floor().min(f64::from(w) - 1.0);
                if x1 < x0 {
                    continue;
                }
                let row = &mut mask[j * wu..(j + 1) * wu];
                for v in &mut row[x0 as usize..=x1 as usize] {
                    *v = 1;
                }
            }
        }
    }
    mask
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode_png(w: u32, h: u32, px: impl Fn(u32, u32) -> [u8; 4]) -> Vec<u8> {
        let mut img = image::RgbaImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                img.put_pixel(x, y, image::Rgba(px(x, y)));
            }
        }
        let mut out = Vec::new();
        img.write_to(&mut Cursor::new(&mut out), ImageFormat::Png).expect("png");
        out
    }

    #[test]
    fn resolve_follows_ezdxf_order_and_refuses_remote_names_without_probing() {
        let (root, abs_str) = if cfg!(windows) { ("C:\\drawings", "D:\\archive\\plan.png") } else { ("/drawings", "/archive/plan.png") };
        let dir = Path::new(root);
        let wanted = dir.join("scans").join("plan.png");
        // relative to the drawing folder
        let got = resolve_path(Some(dir), "scans\\plan.png", &mut |p| p == wanted.as_path());
        assert_eq!(got, Some(wanted.clone()));
        // the file name in the drawing folder when the stored folder does not exist
        let by_name = dir.join("plan.png");
        let got = resolve_path(Some(dir), "D:\\old\\plan.png", &mut |p| p == by_name.as_path());
        assert_eq!(got, Some(by_name.clone()));
        // an absolute name that exists is used as it is
        let abs = Path::new(abs_str);
        let got = resolve_path(Some(dir), abs_str, &mut |p| p == abs);
        assert_eq!(got.as_deref(), Some(abs));
        // nothing found, no drawing folder: no probe at all
        let mut calls = 0;
        assert!(resolve_path(None, "C:\\x\\plan.png", &mut |_| {
            calls += 1;
            true
        })
        .is_none());
        assert_eq!(calls, 0);
        // remote names: refused before any file system access, even when a local file of the same name exists
        for remote in ["\\\\server\\share\\plan.png", "//server/share/plan.png", "\\\\?\\C:\\plan.png", "file:///C:/plan.png", "http://h/plan.png", "   "] {
            let mut calls = 0;
            let r = resolve_path(Some(dir), remote, &mut |_| {
                calls += 1;
                true
            });
            assert!(r.is_none(), "{remote}");
            assert_eq!(calls, 0, "no file access for {remote:?}");
        }
    }

    #[test]
    fn decodes_the_five_formats_and_rejects_others() {
        let png = encode_png(2, 2, |x, y| [(x * 255) as u8, (y * 255) as u8, 7, 255]);
        let r = decode(&png).expect("png");
        assert_eq!((r.width, r.height, r.rgba.len()), (2, 2, 16));
        assert_eq!(&r.rgba[4..8], &[255, 0, 7, 255], "row 0, column 1");

        let mut bmp = Vec::new();
        image::RgbImage::from_fn(3, 2, |_, _| image::Rgb([9, 8, 7])).write_to(&mut Cursor::new(&mut bmp), ImageFormat::Bmp).expect("bmp");
        assert_eq!(decode(&bmp).expect("bmp").rgba[..4], [9, 8, 7, 255]);

        let mut jpg = Vec::new();
        image::RgbImage::from_fn(16, 16, |_, _| image::Rgb([200, 30, 30])).write_to(&mut Cursor::new(&mut jpg), ImageFormat::Jpeg).expect("jpg");
        let j = decode(&jpg).expect("jpeg");
        assert!((i32::from(j.rgba[0]) - 200).abs() <= 4 && j.rgba[3] == 255);

        let mut gif = Vec::new();
        image::RgbaImage::from_pixel(4, 4, image::Rgba([0, 0, 255, 255])).write_to(&mut Cursor::new(&mut gif), ImageFormat::Gif).expect("gif");
        assert_eq!(decode(&gif).expect("gif").rgba[..3], [0, 0, 255]);

        let mut tif = Vec::new();
        image::RgbaImage::from_pixel(5, 3, image::Rgba([1, 2, 3, 4])).write_to(&mut Cursor::new(&mut tif), ImageFormat::Tiff).expect("tiff");
        assert_eq!(decode(&tif).expect("tiff").rgba[..4], [1, 2, 3, 4]);

        assert!(decode(b"GIF89a but truncated").is_none());
        assert!(decode(b"not an image at all").is_none());
    }

    #[test]
    fn missing_and_oversized_files_give_none() {
        assert!(load_file(Path::new("Z:\\does\\not\\exist.png")).is_none());
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("pic.png");
        std::fs::write(&p, encode_png(2, 2, |_, _| [1, 1, 1, 255])).expect("write");
        assert!(load_file(&p).is_some());
        std::fs::write(&p, b"\x89PNG broken").expect("write");
        assert!(load_file(&p).is_none(), "damaged file");
        assert!(load_file(dir.path()).is_none(), "a folder is not a picture");
    }

    #[test]
    fn contrast_fade_and_brightness_follow_ezdxf() {
        // grey 100 and 200 average 150: contrast 0 (factor 0) gives the flat mean, factor 1 keeps the picture
        let mut px = vec![100, 100, 100, 255, 200, 200, 200, 255];
        adjust_colors(&mut px, 50, 0, 0, [0, 0, 0]);
        assert_eq!(&px[..4], &[150, 150, 150, 255]);
        let mut px = vec![100, 100, 100, 255, 200, 200, 200, 255];
        adjust_colors(&mut px, 50, 50, 0, [0, 0, 0]);
        assert_eq!(px, vec![100, 100, 100, 255, 200, 200, 200, 255], "defaults change nothing");
        // fade 50 % towards white
        let mut px = vec![0, 0, 0, 255];
        adjust_colors(&mut px, 50, 50, 50, [255, 255, 255]);
        assert_eq!(px[0], 127);
        // brightness 100: towards white, brightness 0: towards black
        let mut px = vec![0, 0, 0, 255];
        adjust_colors(&mut px, 100, 50, 0, [0, 0, 0]);
        assert_eq!(px[0], 255);
        let mut px = vec![200, 200, 200, 255];
        adjust_colors(&mut px, 0, 50, 0, [0, 0, 0]);
        assert_eq!(px[0], 0);
        // alpha: opaque unless the transparency flag is set; then scaled
        let mut px = vec![1, 2, 3, 40];
        apply_alpha(&mut px, false, 1.0);
        assert_eq!(px[3], 255);
        let mut px = vec![1, 2, 3, 200];
        apply_alpha(&mut px, true, 0.5);
        assert_eq!(px[3], 100);
    }

    #[test]
    fn polygon_mask_matches_pixel_centres() {
        // a rectangle covering pixel centres 0..=2 in both directions (corners at -0.5 and 2.5)
        let rect = vec![vec![[-0.5, -0.5], [2.5, -0.5], [2.5, 2.5], [-0.5, 2.5]]];
        let m = polygons_mask(4, 4, &rect);
        assert_eq!(m, vec![1, 1, 1, 0, 1, 1, 1, 0, 1, 1, 1, 0, 0, 0, 0, 0]);
        // a triangle and a second polygon: union
        let tri = vec![vec![[0.0, 0.0], [3.5, 0.0], [0.0, 3.5]]];
        let m = polygons_mask(4, 4, &tri);
        assert_eq!(m[0], 1);
        assert_eq!(m[5], 1, "(1, 1) is inside");
        assert_eq!(m[3], 1, "(3, 0) is inside");
        assert_eq!(m[3 * 4], 1, "(0, 3) is inside");
        assert_eq!(m[3 * 4 + 3], 0, "(3, 3) lies outside");
        assert_eq!(m[2 * 4 + 2], 0, "(2, 2) lies outside: x + y > 3.5");
        let both = polygons_mask(4, 4, &[rect[0].clone(), vec![[2.5, 2.5], [4.5, 2.5], [4.5, 4.5], [2.5, 4.5]]]);
        assert_eq!(both[3 * 4 + 3], 1, "inside the second polygon");
        assert_eq!(both[0], 1, "still inside the first one");
        // non finite and degenerate polygons are ignored
        assert!(polygons_mask(4, 4, &[vec![[f64::NAN, 0.0], [1.0, 0.0], [0.0, 1.0]], vec![[0.0, 0.0], [1.0, 0.0]]]).iter().all(|&v| v == 0));
    }
}
