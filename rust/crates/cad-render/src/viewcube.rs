//! View cube geometry in pixels: layout, visible faces, hit testing and the view a region leads to.
//!
//! The cube is drawn with an orthographic projection of the camera rotation only. Faces are split like `AIS_ViewCube`
//! into a centre (a standard view along one axis), edge bands (two axes) and corner squares (three axes).
use glam::DVec3;

use crate::camera::{up_for_direction, Camera, StdView, Viewport};

/// Edge length of the cube in logical pixels (`SetSize(55)`).
pub const SIZE_PX: f64 = 55.0;
/// Label font height in logical pixels (`SetFontHeight(11)`).
pub const FONT_PX: f64 = 11.0;
/// Distance of the cube centre from the upper right corner of the viewport, logical pixels (`Vec2i(90, 90)`).
pub const OFFSET_PX: f64 = 90.0;
/// Duration of the animation after a click, seconds (`SetDuration(0.35)`).
pub const ANIMATION_SECS: f64 = 0.35;
/// Width of the edge bands as a fraction of the cube edge.
pub const EDGE_BAND: f64 = 0.18;

/// Direction of a clickable region: components -1, 0 or 1 along X, Y and Z (1 axis = side, 2 = edge, 3 = corner).
pub type Region = [i8; 3];

/// Side labels (`SetBoxSideLabel`): front is -Y, back +Y, top +Z, bottom -Z, left -X, right +X.
pub const LABELS: [(Region, &str); 6] = [
    ([0, -1, 0], "Přední"),
    ([0, 1, 0], "Zadní"),
    ([0, 0, 1], "Horní"),
    ([0, 0, -1], "Spodní"),
    ([-1, 0, 0], "Levý"),
    ([1, 0, 0], "Pravý"),
];

/// Unit vector from the target to the eye for a region, `None` for the all-zero region.
pub fn region_eye_direction(r: Region) -> Option<DVec3> {
    let v = DVec3::new(f64::from(r[0]), f64::from(r[1]), f64::from(r[2]));
    if v == DVec3::ZERO { None } else { Some(v.normalize()) }
}

/// Up vector of the view of a region (Z up, Y up along the Z axis).
pub fn region_up(r: Region) -> DVec3 {
    match region_eye_direction(r) {
        Some(d) => up_for_direction(d),
        None => DVec3::Z,
    }
}

/// The matching standard view for the six sides.
pub fn region_std_view(r: Region) -> Option<StdView> {
    match r {
        [0, -1, 0] => Some(StdView::Front),
        [0, 1, 0] => Some(StdView::Back),
        [0, 0, 1] => Some(StdView::Top),
        [0, 0, -1] => Some(StdView::Bottom),
        [-1, 0, 0] => Some(StdView::Left),
        [1, 0, 0] => Some(StdView::Right),
        _ => None,
    }
}

/// Number of non-zero components: 1 side, 2 edge, 3 corner.
pub fn region_order(r: Region) -> u8 {
    r.iter().filter(|c| **c != 0).count() as u8
}

/// Placement of the cube in the viewport (physical pixels, y down).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CubeLayout {
    pub center: [f64; 2],
    pub size: f64,
}

impl CubeLayout {
    pub fn new(vp: Viewport, dpr: f64) -> Self {
        Self { center: [vp.width - OFFSET_PX * dpr, OFFSET_PX * dpr], size: SIZE_PX * dpr }
    }
}

fn axis(i: usize) -> DVec3 {
    match i {
        0 => DVec3::X,
        1 => DVec3::Y,
        _ => DVec3::Z,
    }
}

/// Maps a point of the unit cube (each coordinate in [-0.5, 0.5]) to pixels for the camera orientation.
pub fn to_screen(cam: &Camera, l: &CubeLayout, p: DVec3) -> [f64; 2] {
    let (r, u) = (cam.right(), cam.up_vec());
    [l.center[0] + p.dot(r) * l.size, l.center[1] - p.dot(u) * l.size]
}

/// One visible side of the cube.
#[derive(Clone, Debug, PartialEq)]
pub struct CubeFace {
    pub side: Region,
    /// Corners in pixels, counter-clockwise on screen.
    pub corners: [[f64; 2]; 4],
    /// Brightness factor 0..1 of the box colour.
    pub shade: f32,
    pub label: &'static str,
    /// Centre of the label in pixels and the screen vectors of one label pixel along text right and text up
    /// (up has y pointing up in label space; screen y is down, so for a head-on face `up = [0, -1]`).
    pub label_center: [f64; 2],
    pub label_right: [f64; 2],
    pub label_up: [f64; 2],
}

fn side_normal(r: Region) -> DVec3 {
    DVec3::new(f64::from(r[0]), f64::from(r[1]), f64::from(r[2]))
}

/// Text frame of a side: `right x up = outward normal`, Z up on the four vertical sides, Y up on top and bottom.
fn label_frame(side: Region) -> (DVec3, DVec3) {
    let n = side_normal(side);
    let up = if side[2] != 0 { DVec3::Y } else { DVec3::Z };
    (up.cross(n), up)
}

fn tangents(a: usize) -> (usize, usize) {
    match a {
        0 => (1, 2),
        1 => (0, 2),
        _ => (0, 1),
    }
}

fn side_axis(side: Region) -> Option<(usize, i8)> {
    (0..3).find(|i| side[*i] != 0).map(|i| (i, side[i]))
}

fn light() -> DVec3 {
    DVec3::new(-0.35, 0.55, 0.75).normalize()
}

/// Sides facing the camera, in the order of [`LABELS`].
pub fn faces(cam: &Camera, l: &CubeLayout) -> Vec<CubeFace> {
    let to_eye = -cam.direction();
    let (right, up) = (cam.right(), cam.up_vec());
    let mut out = Vec::new();
    for (side, label) in LABELS {
        let n = side_normal(side);
        if n.dot(to_eye) <= 1e-9 {
            continue;
        }
        let Some((a, s)) = side_axis(side) else { continue };
        let (b, c) = tangents(a);
        let (ub, uc) = (axis(b), axis(c));
        let center3 = n * 0.5;
        let corner = |sb: f64, sc: f64| to_screen(cam, l, center3 + ub * (0.5 * sb) + uc * (0.5 * sc));
        let mut corners = [corner(-1.0, -1.0), corner(1.0, -1.0), corner(1.0, 1.0), corner(-1.0, 1.0)];
        // make the order counter-clockwise on screen (y down: positive signed area means clockwise visually)
        let area: f64 = (0..4)
            .map(|i| {
                let (p, q) = (corners[i], corners[(i + 1) % 4]);
                p[0] * q[1] - q[0] * p[1]
            })
            .sum();
        if area > 0.0 {
            corners.reverse();
        }
        let nv = DVec3::new(n.dot(right), n.dot(up), n.dot(to_eye));
        let shade = (0.70 + 0.30 * nv.dot(light()).max(0.0)) as f32;
        let (lr, lu) = label_frame(side);
        let _ = s;
        out.push(CubeFace {
            side,
            corners,
            shade,
            label,
            label_center: to_screen(cam, l, center3),
            label_right: [lr.dot(right), -lr.dot(up)],
            label_up: [lu.dot(right), -lu.dot(up)],
        });
    }
    out
}

/// Region under a pixel, `None` outside of the cube.
pub fn hit_test(cam: &Camera, l: &CubeLayout, px: [f64; 2]) -> Option<Region> {
    let to_eye = -cam.direction();
    let (right, up) = (cam.right(), cam.up_vec());
    let xs = (px[0] - l.center[0]) / l.size;
    let ys = (l.center[1] - px[1]) / l.size;
    for (side, _) in LABELS {
        let n = side_normal(side);
        if n.dot(to_eye) <= 1e-9 {
            continue;
        }
        let Some((a, s)) = side_axis(side) else { continue };
        let (b, c) = tangents(a);
        let (ub, uc) = (axis(b), axis(c));
        let x = xs - (n * 0.5).dot(right);
        let y = ys - (n * 0.5).dot(up);
        let (m00, m01, m10, m11) = (ub.dot(right), uc.dot(right), ub.dot(up), uc.dot(up));
        let det = m00 * m11 - m01 * m10;
        if det.abs() < 1e-9 {
            continue;
        }
        let tb = (x * m11 - m01 * y) / det;
        let tc = (m00 * y - m10 * x) / det;
        if tb.abs() > 0.5 + 1e-9 || tc.abs() > 0.5 + 1e-9 {
            continue;
        }
        let class = |t: f64| -> i8 {
            if t.abs() < 0.5 - EDGE_BAND { 0 } else if t > 0.0 { 1 } else { -1 }
        };
        let mut r: Region = [0; 3];
        r[a] = s;
        r[b] = class(tb);
        r[c] = class(tc);
        return Some(r);
    }
    None
}

/// Quads (in pixels) covering a region on the visible faces, for the hover highlight.
pub fn region_quads(cam: &Camera, l: &CubeLayout, region: Region) -> Vec<[[f64; 2]; 4]> {
    let to_eye = -cam.direction();
    let mut out = Vec::new();
    for a in 0..3 {
        if region[a] == 0 {
            continue;
        }
        let n = axis(a) * f64::from(region[a]);
        if n.dot(to_eye) <= 1e-9 {
            continue;
        }
        let (b, c) = tangents(a);
        let range = |v: i8| -> (f64, f64) {
            match v {
                0 => (-(0.5 - EDGE_BAND), 0.5 - EDGE_BAND),
                1 => (0.5 - EDGE_BAND, 0.5),
                _ => (-0.5, -(0.5 - EDGE_BAND)),
            }
        };
        let (b0, b1) = range(region[b]);
        let (c0, c1) = range(region[c]);
        let p = |tb: f64, tc: f64| to_screen(cam, l, n * 0.5 + axis(b) * tb + axis(c) * tc);
        out.push([p(b0, c0), p(b1, c0), p(b1, c1), p(b0, c1)]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout() -> CubeLayout {
        CubeLayout { center: [100.0, 100.0], size: 100.0 }
    }

    fn cam(view: StdView) -> Camera {
        let mut c = Camera::default();
        c.set_view(view);
        c
    }

    #[test]
    fn layout_follows_the_spec_constants() {
        let l = CubeLayout::new(Viewport::new(800.0, 600.0), 1.0);
        assert_eq!(l.center, [710.0, 90.0]);
        assert_eq!(l.size, 55.0);
        let l2 = CubeLayout::new(Viewport::new(1600.0, 1200.0), 2.0);
        assert_eq!(l2.center, [1420.0, 180.0]);
        assert_eq!(l2.size, 110.0);
    }

    #[test]
    fn labels_are_czech() {
        let names: Vec<&str> = LABELS.iter().map(|l| l.1).collect();
        assert_eq!(names, ["Přední", "Zadní", "Horní", "Spodní", "Levý", "Pravý"]);
        assert_eq!(LABELS[0].0, [0, -1, 0]);
    }

    #[test]
    fn front_view_shows_one_face_with_x_to_the_right() {
        let c = cam(StdView::Front);
        let f = faces(&c, &layout());
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].label, "Přední");
        assert_eq!(f[0].side, [0, -1, 0]);
        // the face fills the 100 px square around the centre
        let xs: Vec<f64> = f[0].corners.iter().map(|p| p[0]).collect();
        let ys: Vec<f64> = f[0].corners.iter().map(|p| p[1]).collect();
        assert!((xs.iter().cloned().fold(f64::MAX, f64::min) - 50.0).abs() < 1e-9);
        assert!((xs.iter().cloned().fold(f64::MIN, f64::max) - 150.0).abs() < 1e-9);
        assert!((ys.iter().cloned().fold(f64::MAX, f64::min) - 50.0).abs() < 1e-9);
        assert!((ys.iter().cloned().fold(f64::MIN, f64::max) - 150.0).abs() < 1e-9);
        // text runs right and upwards on screen
        assert!((f[0].label_right[0] - 1.0).abs() < 1e-9 && f[0].label_right[1].abs() < 1e-9);
        assert!(f[0].label_up[0].abs() < 1e-9 && (f[0].label_up[1] + 1.0).abs() < 1e-9);
        assert_eq!(f[0].label_center, [100.0, 100.0]);
    }

    #[test]
    fn iso_shows_three_faces() {
        let c = cam(StdView::Iso);
        let f = faces(&c, &layout());
        let mut sides: Vec<Region> = f.iter().map(|x| x.side).collect();
        sides.sort();
        assert_eq!(sides, vec![[0, -1, 0], [0, 0, 1], [1, 0, 0]]);
        for face in &f {
            assert!(face.shade > 0.5 && face.shade <= 1.0);
            // counter-clockwise on a y-down screen means negative shoelace sum
            let area: f64 = (0..4)
                .map(|i| {
                    let (p, q) = (face.corners[i], face.corners[(i + 1) % 4]);
                    p[0] * q[1] - q[0] * p[1]
                })
                .sum();
            assert!(area < 0.0);
        }
    }

    #[test]
    fn back_view_labels_are_not_mirrored() {
        let c = cam(StdView::Back);
        let f = faces(&c, &layout());
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].label, "Zadní");
        assert!(f[0].label_right[0] > 0.99, "{:?}", f[0].label_right);
    }

    #[test]
    fn hit_test_classifies_centre_edges_and_corners() {
        let c = cam(StdView::Front);
        let l = layout();
        assert_eq!(hit_test(&c, &l, [100.0, 100.0]), Some([0, -1, 0]));
        // upper left corner square: x negative, z positive
        assert_eq!(hit_test(&c, &l, [55.0, 55.0]), Some([-1, -1, 1]));
        // right edge band
        assert_eq!(hit_test(&c, &l, [145.0, 100.0]), Some([1, -1, 0]));
        // bottom edge band
        assert_eq!(hit_test(&c, &l, [100.0, 145.0]), Some([0, -1, -1]));
        assert_eq!(hit_test(&c, &l, [10.0, 10.0]), None);
        assert_eq!(hit_test(&c, &l, [151.0, 100.0]), None);
    }

    #[test]
    fn hit_test_in_iso_view_finds_the_near_corner() {
        let c = cam(StdView::Iso);
        let l = layout();
        assert_eq!(hit_test(&c, &l, [100.0, 100.0]), Some([1, -1, 1]));
        // far outside
        assert_eq!(hit_test(&c, &l, [300.0, 100.0]), None);
    }

    #[test]
    fn hit_test_agrees_with_the_drawn_faces() {
        let c = cam(StdView::Iso);
        let l = layout();
        for face in faces(&c, &l) {
            let cx = face.corners.iter().map(|p| p[0]).sum::<f64>() / 4.0;
            let cy = face.corners.iter().map(|p| p[1]).sum::<f64>() / 4.0;
            let r = hit_test(&c, &l, [cx, cy]).expect("centre of a face");
            assert_eq!(r, face.side);
        }
    }

    #[test]
    fn regions_lead_to_directions() {
        assert_eq!(region_eye_direction([0, 0, 0]), None);
        let d = region_eye_direction([1, -1, 0]).expect("dir");
        assert!((d - DVec3::new(1.0, -1.0, 0.0).normalize()).length() < 1e-12);
        assert_eq!(region_up([0, 0, 1]), DVec3::Y);
        assert_eq!(region_up([1, 1, 1]), DVec3::Z);
        assert_eq!(region_std_view([0, 0, -1]), Some(StdView::Bottom));
        assert_eq!(region_std_view([1, 1, 0]), None);
        assert_eq!(region_order([1, 0, -1]), 2);
        assert_eq!(region_order([1, 1, 1]), 3);
    }

    #[test]
    fn region_quads_cover_the_region() {
        let c = cam(StdView::Front);
        let l = layout();
        let q = region_quads(&c, &l, [0, -1, 0]);
        assert_eq!(q.len(), 1);
        let w = q[0][1][0] - q[0][0][0];
        assert!((w - 100.0 * (1.0 - 2.0 * EDGE_BAND)).abs() < 1e-9, "{w}");
        // an edge region shows on the visible sides only
        let q2 = region_quads(&cam(StdView::Front), &l, [1, -1, 0]);
        assert_eq!(q2.len(), 1);
        let q3 = region_quads(&cam(StdView::Iso), &l, [1, -1, 0]);
        assert_eq!(q3.len(), 2);
    }
}
