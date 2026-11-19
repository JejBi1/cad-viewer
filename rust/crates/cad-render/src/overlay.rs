//! Overlays drawn on top of the model: measurement lines, markers, labels and translucent meshes (centre of gravity,
//! interference volumes, mesh patches), plus the exact measurement styling of `viewer3d.py`.
use crate::colors::{
    Rgb8, AXIS_COLORS, CENTER_LINE_COLOR, COG_COLOR, MEASURE_LABEL_COLOR, MEASURE_LINE_COLOR,
};
use crate::text::{HAlign, VAlign};

/// OCCT marker scale to diameter in logical pixels (`Prs3d_PointAspect(Aspect_TOM_BALL, color, scale)`).
pub const MARKER_PX_PER_SCALE: f32 = 3.0;
/// Label heights in logical pixels: measurement labels 16, centre of gravity 15.
pub const MEASURE_LABEL_PX: f32 = 16.0;
pub const POINT_LABEL_PX: f32 = 15.0;

/// Polyline in world coordinates (mm).
#[derive(Clone, Debug, PartialEq)]
pub struct OverlayLine {
    pub points: Vec<[f64; 3]>,
    pub closed: bool,
    pub color: Rgb8,
    /// Width in logical pixels.
    pub width: f32,
    pub dashed: bool,
}

impl OverlayLine {
    pub fn segment(a: [f64; 3], b: [f64; 3], color: Rgb8, width: f32, dashed: bool) -> Self {
        Self { points: vec![a, b], closed: false, color, width, dashed }
    }
}

/// Ball marker at a world position.
#[derive(Clone, Debug, PartialEq)]
pub struct OverlayPoint {
    pub pos: [f64; 3],
    pub color: Rgb8,
    /// Diameter in logical pixels.
    pub size: f32,
}

impl OverlayPoint {
    /// Marker with an OCCT scale (3.0 for measurement markers, 4.0 for the centre of gravity).
    pub fn ball(pos: [f64; 3], color: Rgb8, scale: f32) -> Self {
        Self { pos, color, size: scale * MARKER_PX_PER_SCALE }
    }
}

/// Text anchored at a world position, constant pixel size.
#[derive(Clone, Debug, PartialEq)]
pub struct OverlayLabel {
    pub pos: [f64; 3],
    pub text: String,
    pub color: Rgb8,
    /// Text height in logical pixels.
    pub height: f32,
    pub h: HAlign,
    pub v: VAlign,
}

/// Translucent triangles in world coordinates, drawn above the model (interference volumes, patches of mesh faces).
#[derive(Clone, Debug, PartialEq)]
pub struct OverlayMesh {
    pub positions: Vec<[f64; 3]>,
    /// Triangle list into `positions`.
    pub indices: Vec<u32>,
    pub color: Rgb8,
    /// Opacity 0..1 (OCCT transparency 0.35 is alpha 0.65).
    pub alpha: f32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Overlays {
    pub meshes: Vec<OverlayMesh>,
    pub lines: Vec<OverlayLine>,
    pub points: Vec<OverlayPoint>,
    pub labels: Vec<OverlayLabel>,
}

/// `f"{d:.2f} mm"` with a decimal comma.
pub fn distance_label(d: f64) -> String {
    format!("{d:.2} mm").replace('.', ",")
}

/// `f"{ang:.2f}°"` with a decimal comma.
pub fn angle_label(deg: f64) -> String {
    format!("{deg:.2}°").replace('.', ",")
}

fn dist(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

fn mid(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0, (a[2] + b[2]) / 2.0]
}

impl Overlays {
    pub fn is_empty(&self) -> bool {
        self.meshes.is_empty() && self.lines.is_empty() && self.points.is_empty() && self.labels.is_empty()
    }

    pub fn clear(&mut self) {
        *self = Overlays::default();
    }

    /// Appends everything of `other`.
    pub fn extend(&mut self, other: Overlays) {
        self.meshes.extend(other.meshes);
        self.lines.extend(other.lines);
        self.points.extend(other.points);
        self.labels.extend(other.labels);
    }

    /// `_mline`: ignored when the end points coincide (< 1e-9).
    pub fn line(&mut self, a: [f64; 3], b: [f64; 3], color: Rgb8, width: f32, dashed: bool) {
        if dist(a, b) >= 1e-9 {
            self.lines.push(OverlayLine::segment(a, b, color, width, dashed));
        }
    }

    /// `_mlabel`: measurement label (170, 0, 0), height 16.
    pub fn measure_label(&mut self, pos: [f64; 3], text: &str) {
        self.labels.push(OverlayLabel {
            pos,
            text: text.to_string(),
            color: MEASURE_LABEL_COLOR,
            height: MEASURE_LABEL_PX,
            h: HAlign::Left,
            v: VAlign::Bottom,
        });
    }

    /// `_marker`: ball of scale 3.
    pub fn marker(&mut self, pos: [f64; 3], color: Rgb8) {
        self.points.push(OverlayPoint::ball(pos, color, 3.0));
    }

    /// `show_point`: ball of scale 4 with a text of height 15 in the same colour.
    pub fn point_with_label(&mut self, pos: [f64; 3], text: &str, color: Rgb8) {
        self.points.push(OverlayPoint::ball(pos, color, 4.0));
        self.labels.push(OverlayLabel {
            pos,
            text: text.to_string(),
            color,
            height: POINT_LABEL_PX,
            h: HAlign::Left,
            v: VAlign::Bottom,
        });
    }

    /// Centre of gravity (`(0, 120, 255)`).
    pub fn center_of_gravity(&mut self, pos: [f64; 3], text: &str) {
        self.point_with_label(pos, text, COG_COLOR);
    }

    /// `draw_relation`: the shortest connection (width 2.5) with its label in the middle and the dashed X / Y / Z
    /// components (width 1.5, axis colours) chained from the first point, plus the dashed centre-to-centre line.
    pub fn relation(&mut self, min: Option<([f64; 3], [f64; 3])>, center: Option<([f64; 3], [f64; 3])>, label: Option<&str>) {
        if let Some((p1, p2)) = min {
            self.line(p1, p2, MEASURE_LINE_COLOR, 2.5, false);
            if let Some(text) = label.filter(|t| !t.is_empty()) {
                self.measure_label(mid(p1, p2), text);
            }
            let mut a = p1;
            for (i, col) in AXIS_COLORS.iter().enumerate() {
                let mut b = a;
                b[i] = p2[i];
                self.line(a, b, *col, 1.5, true);
                a = b;
            }
        }
        if let Some((c1, c2)) = center {
            self.line(c1, c2, CENTER_LINE_COLOR, 1.5, true);
        }
    }

    /// Distance measurement between two points: markers, line and label `"12,34 mm"`.
    pub fn distance(&mut self, p1: [f64; 3], p2: [f64; 3]) {
        self.marker(p1, MEASURE_LINE_COLOR);
        self.marker(p2, MEASURE_LINE_COLOR);
        self.line(p1, p2, MEASURE_LINE_COLOR, 2.5, false);
        self.measure_label(mid(p1, p2), &distance_label(dist(p1, p2)));
    }

    /// Angle measurement: arms vertex-arm_a and vertex-arm_c and the label at the vertex. No label for a degenerate
    /// arm (< 1e-9), like the Python code.
    pub fn angle(&mut self, arm_a: [f64; 3], vertex: [f64; 3], arm_c: [f64; 3]) {
        for p in [arm_a, vertex, arm_c] {
            self.marker(p, MEASURE_LINE_COLOR);
        }
        self.line(vertex, arm_a, MEASURE_LINE_COLOR, 2.5, false);
        self.line(vertex, arm_c, MEASURE_LINE_COLOR, 2.5, false);
        if let Some(deg) = angle_between(arm_a, vertex, arm_c) {
            self.measure_label(vertex, &angle_label(deg));
        }
    }
}

/// Angle in degrees at `vertex` between the two arms, `None` if an arm is shorter than 1e-9.
pub fn angle_between(a: [f64; 3], vertex: [f64; 3], c: [f64; 3]) -> Option<f64> {
    let va = [a[0] - vertex[0], a[1] - vertex[1], a[2] - vertex[2]];
    let vc = [c[0] - vertex[0], c[1] - vertex[1], c[2] - vertex[2]];
    let (la, lc) = (dist(a, vertex), dist(c, vertex));
    if la <= 1e-9 || lc <= 1e-9 {
        return None;
    }
    let cos = (va[0] * vc[0] + va[1] * vc[1] + va[2] * vc[2]) / (la * lc);
    Some(cos.clamp(-1.0, 1.0).acos().to_degrees())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_use_a_decimal_comma() {
        assert_eq!(distance_label(12.3456), "12,35 mm");
        assert_eq!(distance_label(0.0), "0,00 mm");
        assert_eq!(angle_label(90.0), "90,00°");
        assert_eq!(angle_label(33.333), "33,33°");
    }

    #[test]
    fn angle_math() {
        let a = angle_between([1.0, 0.0, 0.0], [0.0; 3], [0.0, 1.0, 0.0]).expect("angle");
        assert!((a - 90.0).abs() < 1e-9);
        let b = angle_between([1.0, 0.0, 0.0], [0.0; 3], [-1.0, 0.0, 0.0]).expect("angle");
        assert!((b - 180.0).abs() < 1e-9);
        assert_eq!(angle_between([0.0; 3], [0.0; 3], [1.0, 0.0, 0.0]), None);
        // tiny rounding above 1 must not produce NaN
        let c = angle_between([1.0, 1e-17, 0.0], [0.0; 3], [1.0, 0.0, 0.0]).expect("angle");
        assert!(c.is_finite());
    }

    #[test]
    fn relation_matches_draw_relation() {
        let mut o = Overlays::default();
        o.relation(Some(([0.0, 0.0, 0.0], [3.0, 4.0, 12.0])), Some(([1.0, 1.0, 1.0], [2.0, 2.0, 2.0])), Some("13,00 mm"));
        // min line + three components + centre line
        assert_eq!(o.lines.len(), 5);
        assert_eq!(o.lines[0].width, 2.5);
        assert!(!o.lines[0].dashed);
        assert_eq!(o.lines[0].color, MEASURE_LINE_COLOR);
        // components: X from (0,0,0) to (3,0,0), then Y to (3,4,0), then Z to (3,4,12), chained
        assert_eq!(o.lines[1].points, vec![[0.0, 0.0, 0.0], [3.0, 0.0, 0.0]]);
        assert_eq!(o.lines[2].points, vec![[3.0, 0.0, 0.0], [3.0, 4.0, 0.0]]);
        assert_eq!(o.lines[3].points, vec![[3.0, 4.0, 0.0], [3.0, 4.0, 12.0]]);
        assert_eq!(o.lines[1].color, AXIS_COLORS[0]);
        assert_eq!(o.lines[2].color, AXIS_COLORS[1]);
        assert_eq!(o.lines[3].color, AXIS_COLORS[2]);
        assert!(o.lines[1..].iter().all(|l| l.dashed && (l.width == 1.5)));
        assert_eq!(o.lines[4].color, CENTER_LINE_COLOR);
        assert_eq!(o.labels.len(), 1);
        assert_eq!(o.labels[0].pos, [1.5, 2.0, 6.0]);
        assert_eq!(o.labels[0].color, MEASURE_LABEL_COLOR);
        assert_eq!(o.labels[0].height, 16.0);
    }

    #[test]
    fn degenerate_components_are_skipped() {
        let mut o = Overlays::default();
        // p2 differs from p1 only in Y: X and Z components have zero length
        o.relation(Some(([1.0, 1.0, 1.0], [1.0, 5.0, 1.0])), None, None);
        assert_eq!(o.lines.len(), 2);
        assert!(o.labels.is_empty());
        let mut e = Overlays::default();
        e.relation(Some(([1.0; 3], [1.0; 3])), None, Some("x"));
        assert!(e.lines.is_empty());
    }

    #[test]
    fn markers_and_points() {
        let mut o = Overlays::default();
        o.center_of_gravity([1.0, 2.0, 3.0], "  T  12,5 kg");
        assert_eq!(o.points[0].size, 12.0);
        assert_eq!(o.points[0].color, [0, 120, 255]);
        assert_eq!(o.labels[0].height, 15.0);
        o.marker([0.0; 3], [220, 0, 0]);
        assert_eq!(o.points[1].size, 9.0);
        assert!(!o.is_empty());
        o.clear();
        assert!(o.is_empty());
    }

    #[test]
    fn distance_and_angle_overlays() {
        let mut o = Overlays::default();
        o.distance([0.0; 3], [3.0, 4.0, 0.0]);
        assert_eq!(o.labels[0].text, "5,00 mm");
        assert_eq!(o.points.len(), 2);
        assert_eq!(o.lines.len(), 1);
        let mut a = Overlays::default();
        a.angle([1.0, 0.0, 0.0], [0.0; 3], [0.0, 1.0, 0.0]);
        assert_eq!(a.labels[0].text, "90,00°");
        assert_eq!(a.lines.len(), 2);
        let mut d = Overlays::default();
        d.angle([0.0; 3], [0.0; 3], [0.0, 1.0, 0.0]);
        assert!(d.labels.is_empty());
        let mut x = Overlays::default();
        x.extend(o);
        x.extend(a);
        assert_eq!(x.lines.len(), 3);
    }
}
