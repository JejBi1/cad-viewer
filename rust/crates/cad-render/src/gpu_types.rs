//! Plain-old-data structs shared between Rust and the WGSL shaders. Sizes are asserted in tests.
use bytemuck::{Pod, Zeroable};

/// Per-frame uniform block (`Globals` in the shaders). All vectors are 16-byte aligned.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct Globals {
    /// `crop * vp_full`: what the mesh shaders multiply with (crop is the identity except in pick passes).
    pub view_proj: [[f32; 4]; 4],
    pub vp_full: [[f32; 4]; 4],
    pub crop: [[f32; 4]; 4],
    pub cam_right: [f32; 4],
    pub cam_up: [f32; 4],
    pub view_dir: [f32; 4],
    /// xyz = eye relative to the scene origin, w = 1 for perspective.
    pub eye: [f32; 4],
    /// Section plane `n.xyz`, `d`: the kept side satisfies `dot(n, p) + d >= 0`.
    pub clip: [f32; 4],
    /// Full viewport width and height in pixels, device pixel ratio.
    pub viewport: [f32; 4],
    /// Orthographic: world half width and height of the view; perspective: tangents of the half angles.
    pub cam_extent: [f32; 4],
    pub default_color: [f32; 4],
    pub edge_color: [f32; 4],
    pub bg_top: [f32; 4],
    pub bg_bottom: [f32; 4],
    /// x = section on, y = wireframe.
    pub flags: [u32; 4],
}

/// Segment drawn as a screen-space quad with round caps.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct LineInst {
    pub p0: [f32; 3],
    /// Width in physical pixels.
    pub width: f32,
    pub p1: [f32; 3],
    /// Bit 0: points are pixel coordinates (y down) instead of world positions.
    pub flags: u32,
    /// RGBA bytes.
    pub color: u32,
    /// Dash pattern in pixels, `dash_on == 0` = solid.
    pub dash_on: f32,
    pub dash_off: f32,
    pub pad: f32,
}

pub const LINE_PIXEL_SPACE: u32 = 1;

/// Round marker (ball) in screen space.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct PointInst {
    pub pos: [f32; 3],
    /// Diameter in physical pixels.
    pub size: f32,
    pub color: u32,
    pub flags: u32,
    pub pad: [u32; 2],
}

/// Vertex of the screen-space layer (text glyph quads and solid shapes share one pipeline; solid shapes sample a white
/// texel of the glyph atlas).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct ScreenVertex {
    pub pos: [f32; 2],
    pub uv: [f32; 2],
    pub color: u32,
}

/// Uniform block of the 2D drawing shaders (`G2`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct Globals2d {
    /// View centre relative to the scene origin, drawing units.
    pub center: [f32; 2],
    /// Viewport size in pixels.
    pub viewport: [f32; 2],
    /// Pixels per drawing unit.
    pub scale: f32,
    /// 0 = line widths in drawing units, 1 = pixels, 2 = pixels times `px_scale`.
    pub width_mode: u32,
    /// Narrowest line drawn, pixels (hairlines).
    pub min_width: f32,
    /// With `width_mode` 2: pixels of a line width of 1 (the device pixel ratio), so that a highlight that is made once
    /// is as wide as a pen drawn by the interface.
    pub px_scale: f32,
    /// A colour of a batch that is `fg_mark` (rgb, `w` 1 when in use) is drawn as `fg` instead (alpha stays), and one that is
    /// `bg_mark` as `bg`: the foreground and the background colour of the layout, which change with the white background
    /// option without the scene being made again (`Renderer2d::set_palette`).
    pub fg_mark: [f32; 4],
    pub fg: [f32; 4],
    pub bg_mark: [f32; 4],
    pub bg: [f32; 4],
}

/// Segment of the 2D drawing.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct LineInst2d {
    pub p0: [f32; 2],
    pub p1: [f32; 2],
    pub width: f32,
    pub color: u32,
}

/// Vertex of a filled triangle of the 2D drawing.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct FillVertex2d {
    pub pos: [f32; 2],
    pub color: u32,
}

/// Column-major f32 matrix from a glam f64 matrix.
pub fn mat4_to_array(m: &glam::DMat4) -> [[f32; 4]; 4] {
    m.as_mat4().to_cols_array_2d()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::size_of;

    #[test]
    fn layouts_match_the_shaders() {
        assert_eq!(size_of::<Globals>(), 3 * 64 + 11 * 16 + 16);
        assert_eq!(size_of::<LineInst>(), 48);
        assert_eq!(size_of::<PointInst>(), 32);
        assert_eq!(size_of::<ScreenVertex>(), 20);
        assert_eq!(size_of::<Globals2d>(), 96);
        assert_eq!(size_of::<LineInst2d>(), 24);
        assert_eq!(size_of::<FillVertex2d>(), 12);
        assert_eq!(size_of::<Globals>() % 16, 0);
    }
}
