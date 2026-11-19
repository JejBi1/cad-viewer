//! Text for the 3D and 2D overlays: a glyph atlas rasterised on the CPU from egui's default font (Ubuntu-Light, which
//! covers the Czech alphabet) and quad generation for the screen-space layer. Czech diacritics in the labels
//! ("Přední", "Řez") and the degree sign work because the atlas is filled per character on demand.
use std::collections::HashMap;

use ab_glyph::{point, Font, FontArc, PxScale, ScaleFont};

use crate::gpu_types::ScreenVertex;

pub const ATLAS_SIZE: u32 = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum HAlign {
    #[default]
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum VAlign {
    Top,
    Center,
    #[default]
    Bottom,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlyphInfo {
    /// Texture rectangle `[u0, v0, u1, v1]`.
    pub uv: [f32; 4],
    /// Offset of the bitmap's upper left corner from the pen position on the baseline (y down).
    pub offset: [f32; 2],
    pub size: [f32; 2],
}

/// R8 coverage atlas with a shelf packer. Cell (0, 0) is a white block used for solid shapes.
pub struct GlyphAtlas {
    font: FontArc,
    pub pixels: Vec<u8>,
    map: HashMap<(char, u32), Option<GlyphInfo>>,
    x: u32,
    y: u32,
    row_h: u32,
    /// Set when `pixels` changed and the GPU copy is stale.
    pub dirty: bool,
}

const WHITE_CELL: u32 = 4;

fn font_key(px: f32) -> u32 {
    (px.max(1.0) * 2.0).round() as u32
}

impl GlyphAtlas {
    /// Atlas with egui's default proportional font; `None` when no font data is built in.
    pub fn new() -> Option<Self> {
        let defs = eframe::egui::FontDefinitions::default();
        let data = defs.font_data.get("Ubuntu-Light").or_else(|| defs.font_data.values().next())?;
        let bytes: &[u8] = (*data.font).as_ref();
        Self::with_font_bytes(bytes.to_vec())
    }

    pub fn with_font_bytes(bytes: Vec<u8>) -> Option<Self> {
        let font = FontArc::try_from_vec(bytes).ok()?;
        let mut pixels = vec![0u8; (ATLAS_SIZE * ATLAS_SIZE) as usize];
        for yy in 0..WHITE_CELL {
            for xx in 0..WHITE_CELL {
                pixels[(yy * ATLAS_SIZE + xx) as usize] = 255;
            }
        }
        Some(Self { font, pixels, map: HashMap::new(), x: WHITE_CELL + 1, y: 0, row_h: WHITE_CELL + 1, dirty: true })
    }

    /// Texture coordinate in the middle of the white block.
    pub fn white_uv(&self) -> [f32; 2] {
        let h = WHITE_CELL as f32 * 0.5 / ATLAS_SIZE as f32;
        [h, h]
    }

    fn scale(px: f32) -> PxScale {
        PxScale::from(font_key(px) as f32 * 0.5)
    }

    /// Distance from the baseline to the top of the line (positive) for a text size in pixels.
    pub fn ascent(&self, px: f32) -> f32 {
        self.font.as_scaled(Self::scale(px)).ascent()
    }

    /// Distance from the baseline to the bottom of the line (negative).
    pub fn descent(&self, px: f32) -> f32 {
        self.font.as_scaled(Self::scale(px)).descent()
    }

    pub fn advance(&self, c: char, px: f32) -> f32 {
        let s = self.font.as_scaled(Self::scale(px));
        s.h_advance(s.glyph_id(c))
    }

    /// Width of a single line of text in pixels.
    pub fn measure(&self, text: &str, px: f32) -> f32 {
        let s = self.font.as_scaled(Self::scale(px));
        let mut w = 0.0;
        let mut prev = None;
        for c in text.chars() {
            let id = s.glyph_id(c);
            if let Some(p) = prev {
                w += s.kern(p, id);
            }
            w += s.h_advance(id);
            prev = Some(id);
        }
        w
    }

    /// Bitmap of a character, rasterised on first use. `None` for characters without an outline (space) and when the
    /// atlas is full.
    pub fn glyph(&mut self, c: char, px: f32) -> Option<GlyphInfo> {
        let key = (c, font_key(px));
        if let Some(g) = self.map.get(&key) {
            return *g;
        }
        let g = self.rasterise(c, px);
        self.map.insert(key, g);
        g
    }

    fn rasterise(&mut self, c: char, px: f32) -> Option<GlyphInfo> {
        let scale = Self::scale(px);
        let glyph = self.font.glyph_id(c).with_scale_and_position(scale, point(0.0, 0.0));
        let og = self.font.outline_glyph(glyph)?;
        let b = og.px_bounds();
        let w = (b.max.x - b.min.x).ceil().max(1.0) as u32;
        let h = (b.max.y - b.min.y).ceil().max(1.0) as u32;
        if w + 2 > ATLAS_SIZE || h + 2 > ATLAS_SIZE {
            return None;
        }
        if self.x + w + 1 > ATLAS_SIZE {
            self.x = 0;
            self.y += self.row_h;
            self.row_h = 0;
        }
        if self.y + h + 1 > ATLAS_SIZE {
            return None; // atlas full: the character is skipped
        }
        let (ox, oy) = (self.x, self.y);
        og.draw(|gx, gy, cov| {
            if gx < w && gy < h {
                let idx = ((oy + gy) * ATLAS_SIZE + ox + gx) as usize;
                self.pixels[idx] = (cov.clamp(0.0, 1.0) * 255.0).round() as u8;
            }
        });
        self.x += w + 1;
        self.row_h = self.row_h.max(h + 1);
        self.dirty = true;
        let s = ATLAS_SIZE as f32;
        Some(GlyphInfo {
            uv: [ox as f32 / s, oy as f32 / s, (ox + w) as f32 / s, (oy + h) as f32 / s],
            offset: [b.min.x, b.min.y],
            size: [w as f32, h as f32],
        })
    }

    /// Number of distinct glyph bitmaps cached.
    pub fn cached(&self) -> usize {
        self.map.values().filter(|g| g.is_some()).count()
    }
}

/// Text frame: `right` and `up` are the screen vectors of one pixel of text advance and of text height. Normal
/// horizontal text on a y-down screen has `right = [1, 0]`, `up = [0, -1]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextFrame {
    pub right: [f32; 2],
    pub up: [f32; 2],
}

impl Default for TextFrame {
    fn default() -> Self {
        Self { right: [1.0, 0.0], up: [0.0, -1.0] }
    }
}

/// Appends the glyph quads of one line of text.
pub fn push_text(
    out: &mut Vec<ScreenVertex>,
    atlas: &mut GlyphAtlas,
    text: &str,
    anchor: [f32; 2],
    px: f32,
    color: u32,
    h: HAlign,
    v: VAlign,
    frame: TextFrame,
) {
    if text.is_empty() || px < 1.0 {
        return;
    }
    let width = atlas.measure(text, px);
    let ascent = atlas.ascent(px);
    let descent = atlas.descent(px);
    let x0 = match h {
        HAlign::Left => 0.0,
        HAlign::Center => -width * 0.5,
        HAlign::Right => -width,
    };
    // baseline position along the text "up" axis relative to the anchor
    let base_up = match v {
        VAlign::Bottom => -descent,
        VAlign::Center => -(ascent + descent) * 0.5,
        VAlign::Top => -ascent,
    };
    let to_screen = |a: f32, b: f32| -> [f32; 2] {
        [anchor[0] + frame.right[0] * a + frame.up[0] * b, anchor[1] + frame.right[1] * a + frame.up[1] * b]
    };
    let axis_aligned = frame == TextFrame::default();
    let mut pen = x0;
    let font = atlas.font.clone();
    let s = font.as_scaled(GlyphAtlas::scale(px));
    let mut prev = None;
    for c in text.chars() {
        let id = s.glyph_id(c);
        if let Some(p) = prev {
            pen += s.kern(p, id);
        }
        let adv = s.h_advance(id);
        prev = Some(id);
        let Some(g) = atlas.glyph(c, px) else {
            pen += adv;
            continue;
        };
        let (mut gx0, mut gy_top) = (pen + g.offset[0], base_up - g.offset[1]);
        if axis_aligned {
            // snap the bitmap to whole pixels for crisp text
            let tlx = (anchor[0] + gx0).round();
            let tly = (anchor[1] - gy_top).round();
            gx0 = tlx - anchor[0];
            gy_top = anchor[1] - tly;
        }
        let gx1 = gx0 + g.size[0];
        let gy_bot = gy_top - g.size[1];
        let tl = to_screen(gx0, gy_top);
        let tr = to_screen(gx1, gy_top);
        let br = to_screen(gx1, gy_bot);
        let bl = to_screen(gx0, gy_bot);
        let (u0, v0, u1, v1) = (g.uv[0], g.uv[1], g.uv[2], g.uv[3]);
        let vert = |p: [f32; 2], u: f32, vv: f32| ScreenVertex { pos: p, uv: [u, vv], color };
        out.extend_from_slice(&[
            vert(tl, u0, v0),
            vert(tr, u1, v0),
            vert(br, u1, v1),
            vert(tl, u0, v0),
            vert(br, u1, v1),
            vert(bl, u0, v1),
        ]);
        pen += adv;
    }
}

/// Appends a filled polygon (convex, as a fan) in solid colour.
pub fn push_polygon(out: &mut Vec<ScreenVertex>, atlas: &GlyphAtlas, pts: &[[f32; 2]], color: u32) {
    let uv = atlas.white_uv();
    for i in 1..pts.len().saturating_sub(1) {
        for p in [pts[0], pts[i], pts[i + 1]] {
            out.push(ScreenVertex { pos: p, uv, color });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn atlas() -> GlyphAtlas {
        GlyphAtlas::new().expect("egui default font")
    }

    #[test]
    fn czech_letters_have_bitmaps() {
        let mut a = atlas();
        for c in "Přední Zadní Horní Spodní Levý Pravý Řez ěščřžýáíéůúďťň°ĚŠČŘŽ".chars() {
            if c == ' ' {
                assert!(a.glyph(c, 13.0).is_none());
                continue;
            }
            let g = a.glyph(c, 13.0).unwrap_or_else(|| panic!("no glyph for {c:?}"));
            assert!(g.size[0] >= 1.0 && g.size[1] >= 1.0);
        }
        assert!(a.cached() > 20);
        assert!(a.dirty);
    }

    #[test]
    fn glyph_bitmaps_contain_ink_and_are_cached() {
        let mut a = atlas();
        let g = a.glyph('Ř', 24.0).expect("glyph");
        let n_before = a.cached();
        let g2 = a.glyph('Ř', 24.0).expect("glyph");
        assert_eq!(g, g2);
        assert_eq!(a.cached(), n_before);
        let s = ATLAS_SIZE as f32;
        let (x0, y0) = ((g.uv[0] * s).round() as u32, (g.uv[1] * s).round() as u32);
        let mut ink = 0u32;
        for yy in 0..g.size[1] as u32 {
            for xx in 0..g.size[0] as u32 {
                ink += u32::from(a.pixels[((y0 + yy) * ATLAS_SIZE + x0 + xx) as usize] > 128);
            }
        }
        assert!(ink > 20, "ink {ink}");
        // different size = different bitmap
        let g3 = a.glyph('Ř', 12.0).expect("glyph");
        assert!(g3.size[0] < g.size[0]);
    }

    #[test]
    fn white_cell_is_solid() {
        let a = atlas();
        let [u, v] = a.white_uv();
        let (x, y) = ((u * ATLAS_SIZE as f32) as usize, (v * ATLAS_SIZE as f32) as usize);
        assert_eq!(a.pixels[y * ATLAS_SIZE as usize + x], 255);
    }

    #[test]
    fn measure_grows_with_text_and_size() {
        let a = atlas();
        let w1 = a.measure("mm", 13.0);
        let w2 = a.measure("500 mm", 13.0);
        let w3 = a.measure("500 mm", 26.0);
        assert!(w1 > 0.0 && w2 > w1);
        assert!((w3 / w2 - 2.0).abs() < 0.1, "{w3} {w2}");
        assert_eq!(a.measure("", 13.0), 0.0);
        assert!(a.ascent(13.0) > 8.0 && a.descent(13.0) < 0.0);
    }

    #[test]
    fn push_text_builds_quads_and_aligns() {
        let mut a = atlas();
        let mut v = Vec::new();
        push_text(&mut v, &mut a, "Ab", [100.0, 50.0], 16.0, 0xFF00_00FF, HAlign::Left, VAlign::Bottom, TextFrame::default());
        assert_eq!(v.len(), 12);
        let xmin = v.iter().map(|p| p.pos[0]).fold(f32::MAX, f32::min);
        let xmax = v.iter().map(|p| p.pos[0]).fold(f32::MIN, f32::max);
        let ymax = v.iter().map(|p| p.pos[1]).fold(f32::MIN, f32::max);
        assert!(xmin >= 99.0 && xmax <= 100.0 + a.measure("Ab", 16.0) + 1.0);
        // bottom aligned: the lowest ink is near the anchor (descent of 'A' and 'b' is zero)
        assert!(ymax <= 50.0 + 1.5 && ymax > 40.0, "{ymax}");

        let mut c = Vec::new();
        push_text(&mut c, &mut a, "Ab", [100.0, 50.0], 16.0, 0xFF, HAlign::Center, VAlign::Center, TextFrame::default());
        let cx = (c.iter().map(|p| p.pos[0]).fold(f32::MAX, f32::min) + c.iter().map(|p| p.pos[0]).fold(f32::MIN, f32::max)) / 2.0;
        assert!((cx - 100.0).abs() < 3.0, "{cx}");
        let cy = (c.iter().map(|p| p.pos[1]).fold(f32::MAX, f32::min) + c.iter().map(|p| p.pos[1]).fold(f32::MIN, f32::max)) / 2.0;
        assert!((cy - 50.0).abs() < 4.0, "{cy}");

        let mut none = Vec::new();
        push_text(&mut none, &mut a, "", [0.0, 0.0], 16.0, 0, HAlign::Left, VAlign::Top, TextFrame::default());
        push_text(&mut none, &mut a, "x", [0.0, 0.0], 0.2, 0, HAlign::Left, VAlign::Top, TextFrame::default());
        assert!(none.is_empty());
    }

    #[test]
    fn rotated_frame_maps_the_text_axis() {
        let mut a = atlas();
        let mut v = Vec::new();
        // text running down the screen (right = +y), up = +x
        let frame = TextFrame { right: [0.0, 1.0], up: [1.0, 0.0] };
        push_text(&mut v, &mut a, "IIIIIIIIII", [0.0, 0.0], 20.0, 1, HAlign::Left, VAlign::Bottom, frame);
        let ys: Vec<f32> = v.iter().map(|p| p.pos[1]).collect();
        let xs: Vec<f32> = v.iter().map(|p| p.pos[0]).collect();
        let spread_y = ys.iter().cloned().fold(f32::MIN, f32::max) - ys.iter().cloned().fold(f32::MAX, f32::min);
        let spread_x = xs.iter().cloned().fold(f32::MIN, f32::max) - xs.iter().cloned().fold(f32::MAX, f32::min);
        assert!(spread_y > spread_x);
    }

    #[test]
    fn polygon_fan() {
        let a = atlas();
        let mut v = Vec::new();
        push_polygon(&mut v, &a, &[[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]], 5);
        assert_eq!(v.len(), 6);
        assert!(v.iter().all(|p| p.uv == a.white_uv() && p.color == 5));
        let mut none = Vec::new();
        push_polygon(&mut none, &a, &[[0.0, 0.0], [1.0, 0.0]], 5);
        assert!(none.is_empty());
    }
}
