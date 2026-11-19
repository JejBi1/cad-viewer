//! Overlays of the 2D drawing view: selection highlight, measurement and snap marker (`DrawingView`).
use crate::colors::{Rgb8, MEASURE_2D, SEL_2D, SNAP_2D};
use crate::text::{HAlign, VAlign};

/// Measurement pen width (cosmetic, pixels), snap square size and pen width, measurement dot diameter.
pub const MEASURE_PEN_PX: f32 = 2.0;
pub const SNAP_SQUARE_PX: f32 = 10.0;
pub const SNAP_PEN_PX: f32 = 2.0;
pub const MEASURE_DOT_PX: f32 = 8.0;
/// Text of measurement labels (11 pt at 96 dpi).
pub const MEASURE_TEXT_PX: f32 = 11.0 * 96.0 / 72.0;
/// Minimum width of the selection highlight (`max(2.0, ...)`).
pub const SELECTION_MIN_PX: f32 = 2.0;

/// Polyline in drawing units, drawn with a constant pixel width.
#[derive(Clone, Debug, PartialEq)]
pub struct Line2d {
    pub points: Vec<[f64; 2]>,
    pub color: Rgb8,
    pub width_px: f32,
    /// Qt `DashLine`: 4 pen widths on, 2 off.
    pub dashed: bool,
}

/// Filled circle of constant pixel size.
#[derive(Clone, Debug, PartialEq)]
pub struct Dot2d {
    pub pos: [f64; 2],
    pub diameter_px: f32,
    pub color: Rgb8,
}

/// Square outline of constant pixel size.
#[derive(Clone, Debug, PartialEq)]
pub struct Square2d {
    pub pos: [f64; 2],
    pub size_px: f32,
    pub width_px: f32,
    pub color: Rgb8,
}

/// Text at a drawing position with constant pixel size.
#[derive(Clone, Debug, PartialEq)]
pub struct Text2d {
    pub pos: [f64; 2],
    pub text: String,
    pub color: Rgb8,
    pub size_px: f32,
    pub h: HAlign,
    pub v: VAlign,
}

/// Translucent filled triangles in drawing units (highlighted solids, hatches).
#[derive(Clone, Debug, PartialEq)]
pub struct Tris2d {
    /// Point triples.
    pub triangles: Vec<[f64; 2]>,
    pub color: Rgb8,
    pub alpha: f32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Overlays2d {
    pub triangles: Vec<Tris2d>,
    pub lines: Vec<Line2d>,
    pub dots: Vec<Dot2d>,
    pub squares: Vec<Square2d>,
    pub texts: Vec<Text2d>,
}

impl Overlays2d {
    pub fn is_empty(&self) -> bool {
        self.triangles.is_empty()
            && self.lines.is_empty()
            && self.dots.is_empty()
            && self.squares.is_empty()
            && self.texts.is_empty()
    }

    pub fn clear(&mut self) {
        *self = Overlays2d::default();
    }

    /// Selected entity: the polyline in orange (255, 140, 0) at least 2 px wide (`SEL`).
    pub fn selection(&mut self, points: Vec<[f64; 2]>, width_px: f32) {
        self.lines.push(Line2d { points, color: SEL_2D, width_px: width_px.max(SELECTION_MIN_PX), dashed: false });
    }

    /// Measurement pen (230, 30, 30), 2 px.
    pub fn measure_line(&mut self, a: [f64; 2], b: [f64; 2], dashed: bool) {
        self.lines.push(Line2d {
            points: vec![a, b],
            color: MEASURE_2D,
            width_px: MEASURE_PEN_PX,
            dashed,
        });
    }

    /// Measurement point: red dot of 8 px.
    pub fn measure_dot(&mut self, pos: [f64; 2]) {
        self.dots.push(Dot2d { pos, diameter_px: MEASURE_DOT_PX, color: MEASURE_2D });
    }

    /// Measurement label in red.
    pub fn measure_text(&mut self, pos: [f64; 2], text: &str) {
        self.texts.push(Text2d {
            pos,
            text: text.to_string(),
            color: MEASURE_2D,
            size_px: MEASURE_TEXT_PX,
            h: HAlign::Left,
            v: VAlign::Top,
        });
    }

    /// Snap marker: 10 px square outline in (0, 200, 255), 2 px.
    pub fn snap_square(&mut self, pos: [f64; 2]) {
        self.squares.push(Square2d { pos, size_px: SNAP_SQUARE_PX, width_px: SNAP_PEN_PX, color: SNAP_2D });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_styles() {
        let mut o = Overlays2d::default();
        assert!(o.is_empty());
        o.selection(vec![[0.0, 0.0], [1.0, 1.0]], 0.5);
        assert_eq!(o.lines[0].color, [255, 140, 0]);
        assert_eq!(o.lines[0].width_px, 2.0);
        o.selection(vec![[0.0, 0.0], [1.0, 1.0]], 3.5);
        assert_eq!(o.lines[1].width_px, 3.5);
        o.measure_line([0.0, 0.0], [1.0, 0.0], true);
        assert_eq!(o.lines[2].color, [230, 30, 30]);
        assert!(o.lines[2].dashed);
        o.measure_dot([1.0, 1.0]);
        assert_eq!(o.dots[0].diameter_px, 8.0);
        o.snap_square([2.0, 2.0]);
        assert_eq!(o.squares[0].color, [0, 200, 255]);
        assert_eq!(o.squares[0].size_px, 10.0);
        o.measure_text([0.0, 0.0], "12,50");
        assert!((o.texts[0].size_px - 14.667).abs() < 0.01);
        assert!(!o.is_empty());
        o.clear();
        assert!(o.is_empty());
    }
}
