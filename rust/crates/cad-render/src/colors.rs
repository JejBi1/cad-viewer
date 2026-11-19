//! Colour constants of the viewer (SPEC section 7) and colour-space helpers.
//!
//! All shading is done in gamma (sRGB-encoded) space, like the software thumbnail renderer of the Python version,
//! so every colour here is an sRGB byte triple.
use cad_types::Rgb;

pub type Rgb8 = [u8; 3];

/// Default part colour (`main.py:284`).
pub const DEFAULT_PART_COLOR: Rgb8 = [200, 204, 210];
/// Edge line colour of the interactive view (`viewer3d.py:305`).
pub const EDGE_COLOR: Rgb8 = [25, 25, 30];
/// Edge line colour of the OpenGL thumbnail render (`thumbs.py:90`).
pub const THUMB_EDGE_COLOR: Rgb8 = [40, 40, 46];
/// Hover highlight of whole parts (`viewer3d.py:309`).
pub const HIGHLIGHT_COLOR: Rgb8 = [80, 200, 255];
/// Selection colour (`viewer3d.py:312`).
pub const SELECTION_COLOR: Rgb8 = [255, 140, 0];
pub const PICK_COLOR: Rgb8 = [255, 140, 0];
pub const HOVER_COLOR: Rgb8 = [0, 170, 255];
/// X, Y, Z component colours of measurements.
pub const AXIS_COLORS: [Rgb8; 3] = [[220, 40, 40], [30, 160, 50], [40, 90, 230]];
/// Marker of a picked point snapped to a mesh vertex.
pub const SNAPPED_MARKER_COLOR: Rgb8 = [0, 150, 255];
/// Centre-of-gravity marker and text.
pub const COG_COLOR: Rgb8 = [0, 120, 255];
/// Interference volumes.
pub const INTERFERENCE_COLOR: Rgb8 = [230, 20, 20];
/// Measurement label text and the default measurement line.
pub const MEASURE_LABEL_COLOR: Rgb8 = [170, 0, 0];
pub const MEASURE_LINE_COLOR: Rgb8 = [220, 0, 0];
/// Dashed centre-to-centre line.
pub const CENTER_LINE_COLOR: Rgb8 = [120, 120, 130];
/// Gradient background (top, bottom).
pub const BG_LIGHT: (Rgb8, Rgb8) = ([230, 235, 242], [150, 162, 180]);
pub const BG_DARK: (Rgb8, Rgb8) = ([50, 54, 62], [20, 22, 26]);
/// Scale bar colours.
pub const SCALE_BAR_DARK_BG: Rgb8 = [235, 235, 235];
pub const SCALE_BAR_LIGHT_BG: Rgb8 = [30, 32, 38];
/// View cube box.
pub const VIEW_CUBE_BOX: Rgb8 = [225, 228, 235];

// ---- 2D view (view2d.py)
/// Selected entity overlay (`DrawingView.SEL`).
pub const SEL_2D: Rgb8 = [255, 140, 0];
/// Measurement pen, text and dots.
pub const MEASURE_2D: Rgb8 = [230, 30, 30];
/// Snap square.
pub const SNAP_2D: Rgb8 = [0, 200, 255];
/// Ruler cursor tick.
pub const RULER_CURSOR_2D: Rgb8 = [230, 60, 60];
/// ezdxf's default model space background (`BackgroundPolicy.DEFAULT`), used when the white background is off.
pub const BG_2D_DARK: Rgb8 = [0x21, 0x28, 0x30];
pub const BG_2D_LIGHT: Rgb8 = [255, 255, 255];
/// Border of the 2D thumbnail paper.
pub const PAPER_BORDER_2D: Rgb8 = [150, 150, 150];

/// sRGB-encoded value (0..1) of a linear component (`thumbs._lin2srgb`).
pub fn linear_to_srgb(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.003_130_8 { 12.92 * v } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
}

/// Linear value (0..1) of an sRGB-encoded component.
pub fn srgb_to_linear(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.040_45 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

/// sRGB bytes of a linear document colour.
pub fn rgb_to_srgb8(c: Rgb) -> Rgb8 {
    [to_u8(linear_to_srgb(c.0)), to_u8(linear_to_srgb(c.1)), to_u8(linear_to_srgb(c.2))]
}

pub fn to_u8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// sRGB bytes -> float triple 0..1 (still gamma-encoded).
pub fn to_f32(c: Rgb8) -> [f32; 3] {
    [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0]
}

/// Packs bytes as little-endian RGBA (R in the lowest byte), the layout of a `Unorm8x4` vertex attribute.
pub fn pack_rgba(c: Rgb8, a: u8) -> u32 {
    u32::from(c[0]) | (u32::from(c[1]) << 8) | (u32::from(c[2]) << 16) | (u32::from(a) << 24)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb_roundtrip() {
        for i in 0..=255u32 {
            let v = i as f32 / 255.0;
            let back = linear_to_srgb(srgb_to_linear(v));
            assert!((back - v).abs() < 1e-4, "{i}: {back} vs {v}");
        }
    }

    #[test]
    fn known_values() {
        assert_eq!(rgb_to_srgb8(Rgb(0.0, 0.0, 0.0)), [0, 0, 0]);
        assert_eq!(rgb_to_srgb8(Rgb(1.0, 1.0, 1.0)), [255, 255, 255]);
        // 0.2140 linear is sRGB 0.5 -> 127 or 128 after rounding
        assert!(matches!(rgb_to_srgb8(Rgb(0.214, 0.214, 0.214))[0], 127 | 128));
        assert_eq!(rgb_to_srgb8(Rgb(0.0, 0.0, 0.0)), [0, 0, 0]);
        assert_eq!(rgb_to_srgb8(Rgb(2.0, -1.0, 0.5))[0], 255);
        assert_eq!(rgb_to_srgb8(Rgb(2.0, -1.0, 0.5))[1], 0);
    }

    #[test]
    fn packing_is_little_endian_rgba() {
        let p = pack_rgba([1, 2, 3], 4);
        assert_eq!(p.to_le_bytes(), [1, 2, 3, 4]);
    }

    #[test]
    fn spec_constants() {
        assert_eq!(DEFAULT_PART_COLOR, [200, 204, 210]);
        assert_eq!(SELECTION_COLOR, [255, 140, 0]);
        assert_eq!(HIGHLIGHT_COLOR, [80, 200, 255]);
        assert_eq!(BG_LIGHT, ([230, 235, 242], [150, 162, 180]));
        assert_eq!(BG_DARK, ([50, 54, 62], [20, 22, 26]));
        assert_eq!(AXIS_COLORS[2], [40, 90, 230]);
    }
}
