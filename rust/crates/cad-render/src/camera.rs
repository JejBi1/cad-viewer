//! Camera maths of the 3D view as plain functions on a plain struct (no GPU, no UI).
//!
//! Conventions: world is Z up, millimetres, right-handed. Pixel coordinates are physical pixels with the origin in the
//! upper left corner of the viewport and y pointing down. The behaviour follows `viewer3d.py` (SPEC section 7).
use cad_types::Aabb;
use glam::{DMat3, DMat4, DQuat, DVec3, DVec4};

/// Vertical field of view of the perspective projection, degrees (OCCT default of `Graphic3d_Camera`).
pub const DEFAULT_FOV_Y_DEG: f64 = 45.0;
/// `view.FitAll(0.04)` of `fit_all`.
pub const FIT_ALL_MARGIN: f64 = 0.04;
/// `view.FitAll(box, 0.15)` of `fit_box` (zoom to selection).
pub const FIT_SELECTION_MARGIN: f64 = 0.15;
/// Radians per physical pixel and unit of device pixel ratio of the orbit drag (`k = 0.006 / dpr`).
pub const ORBIT_RAD_PER_PX: f64 = 0.006;
/// 3D mouse in object mode: rotation at full deflection (rad/s), pan (px/s) and zoom (e-folds/s).
pub const SM_ROT: f64 = 150.0 * std::f64::consts::PI / 180.0;
pub const SM_PAN: f64 = 1100.0;
pub const SM_ZOOM: f64 = 2.2;

/// Size of the drawing area in physical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    pub width: f64,
    pub height: f64,
}

impl Viewport {
    pub fn new(width: f64, height: f64) -> Self {
        Self { width: width.max(1.0), height: height.max(1.0) }
    }

    pub fn aspect(&self) -> f64 {
        self.width / self.height
    }

    /// The smaller side; the orthographic scale is measured along it.
    pub fn min_side(&self) -> f64 {
        self.width.min(self.height)
    }
}

/// Standard views of the toolbar, the number keys and the view cube sides (`ORIENT` in `viewer3d.py`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StdView {
    Iso,
    Front,
    Back,
    Top,
    Bottom,
    Left,
    Right,
}

impl StdView {
    pub const ALL: [StdView; 7] =
        [StdView::Iso, StdView::Front, StdView::Back, StdView::Top, StdView::Bottom, StdView::Left, StdView::Right];

    /// Key of `ORIENT` in the Python code.
    pub fn key(self) -> &'static str {
        match self {
            StdView::Iso => "iso",
            StdView::Front => "front",
            StdView::Back => "back",
            StdView::Top => "top",
            StdView::Bottom => "bottom",
            StdView::Left => "left",
            StdView::Right => "right",
        }
    }

    pub fn from_key(key: &str) -> Option<StdView> {
        StdView::ALL.into_iter().find(|v| v.key() == key)
    }

    /// Unit vector from the target towards the eye (`V3d_TypeOfOrientation_Zup_*`: AxoRight = +X -Y +Z, Front = -Y,
    /// Back = +Y, Top = +Z, Bottom = -Z, Left = -X, Right = +X).
    pub fn eye_direction(self) -> DVec3 {
        match self {
            StdView::Iso => DVec3::new(1.0, -1.0, 1.0).normalize(),
            StdView::Front => DVec3::NEG_Y,
            StdView::Back => DVec3::Y,
            StdView::Top => DVec3::Z,
            StdView::Bottom => DVec3::NEG_Z,
            StdView::Left => DVec3::NEG_X,
            StdView::Right => DVec3::X,
        }
    }

    /// Up vector of the screen: Z, except for the views along Z where Y is up.
    pub fn up(self) -> DVec3 {
        match self {
            StdView::Top | StdView::Bottom => DVec3::Y,
            _ => DVec3::Z,
        }
    }
}

/// Up vector for an arbitrary eye direction (Z up; Y up when looking along Z).
pub fn up_for_direction(to_eye: DVec3) -> DVec3 {
    let d = to_eye.normalize_or_zero();
    if d.cross(DVec3::Z).length_squared() < 1e-12 { DVec3::Y } else { DVec3::Z }
}

/// Perspective or orthographic camera. In orthographic mode `scale` is the world size of the shorter viewport side; in
/// perspective mode the visible height at the centre plane is `2 * distance * tan(fov / 2)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub eye: DVec3,
    pub center: DVec3,
    pub up: DVec3,
    pub perspective: bool,
    pub scale: f64,
    pub fov_y_deg: f64,
}

impl Default for Camera {
    fn default() -> Self {
        Camera::looking(StdView::Iso, DVec3::ZERO, 1000.0, 1000.0)
    }
}

impl Camera {
    /// Camera looking at `center` from the direction of a standard view at `distance`.
    pub fn looking(view: StdView, center: DVec3, distance: f64, scale: f64) -> Self {
        Camera {
            eye: center + view.eye_direction() * distance,
            center,
            up: view.up(),
            perspective: false,
            scale,
            fov_y_deg: DEFAULT_FOV_Y_DEG,
        }
    }

    /// Unit vector from the eye to the centre.
    pub fn direction(&self) -> DVec3 {
        let d = self.center - self.eye;
        if d.length_squared() < 1e-24 { DVec3::Y } else { d.normalize() }
    }

    pub fn distance(&self) -> f64 {
        self.eye.distance(self.center)
    }

    /// Screen right vector (unit).
    pub fn right(&self) -> DVec3 {
        let r = self.direction().cross(self.up);
        if r.length_squared() < 1e-24 {
            let d = self.direction();
            let alt = if d.z.abs() > 0.9 { DVec3::Y } else { DVec3::Z };
            d.cross(alt).normalize()
        } else {
            r.normalize()
        }
    }

    /// Screen up vector (unit, orthogonal to the view direction).
    pub fn up_vec(&self) -> DVec3 {
        self.right().cross(self.direction()).normalize()
    }

    fn orthonormalize(&mut self) {
        self.up = self.up_vec();
    }

    pub fn tan_half_fov(&self) -> f64 {
        (self.fov_y_deg.to_radians() * 0.5).tan()
    }

    /// Half sizes of the visible region on the plane through the centre, perpendicular to the view direction.
    pub fn half_size(&self, vp: Viewport) -> (f64, f64) {
        let aspect = vp.aspect();
        if self.perspective {
            let hy = self.distance() * self.tan_half_fov();
            (hy * aspect, hy)
        } else if aspect >= 1.0 {
            let hy = self.scale * 0.5;
            (hy * aspect, hy)
        } else {
            let hx = self.scale * 0.5;
            (hx, hx / aspect)
        }
    }

    /// World length of one pixel on the plane through the centre (`world_per_pixel` of the Python viewer).
    pub fn world_per_pixel(&self, vp: Viewport) -> f64 {
        let (_, hy) = self.half_size(vp);
        2.0 * hy / vp.height
    }

    /// Orthographic units per pixel as used by the scale bar: `scale / min(width, height)`.
    pub fn ortho_units_per_pixel(&self, vp: Viewport) -> f64 {
        self.scale / vp.min_side()
    }

    /// Switch the projection keeping the apparent size at the centre plane.
    pub fn set_perspective(&mut self, on: bool, vp: Viewport) {
        if on == self.perspective {
            return;
        }
        if on {
            // visible height of the orthographic view at the centre plane
            let h = if vp.aspect() >= 1.0 { self.scale } else { self.scale / vp.aspect() };
            let dist = (h / (2.0 * self.tan_half_fov())).max(1e-9);
            self.eye = self.center - self.direction() * dist;
        } else {
            let h = 2.0 * self.distance() * self.tan_half_fov();
            self.scale = if vp.aspect() >= 1.0 { h } else { h * vp.aspect() };
        }
        self.perspective = on;
    }

    /// Standard orientation about the current centre at the current distance (`SetProj`; callers fit afterwards).
    pub fn set_view(&mut self, view: StdView) {
        self.set_direction(view.eye_direction(), view.up());
    }

    /// Looks at the centre from `to_eye` (a direction, not necessarily unit) with the given up hint.
    pub fn set_direction(&mut self, to_eye: DVec3, up: DVec3) {
        let dist = self.distance().max(1e-9);
        let d = to_eye.normalize_or_zero();
        if d == DVec3::ZERO {
            return;
        }
        self.eye = self.center + d * dist;
        self.up = up;
        self.orthonormalize();
    }

    /// View matrix with the world shifted by `origin` (the renderer keeps coordinates relative to the model centre).
    pub fn view_matrix(&self, origin: DVec3) -> DMat4 {
        glam::dcamera::rh::view::look_at_mat4(self.eye - origin, self.center - origin, self.up_vec())
    }

    /// Near and far distance along the view direction that enclose a bounding sphere (`ZFitAll`).
    pub fn depth_range(&self, sphere_center: DVec3, radius: f64) -> (f64, f64) {
        let d = (sphere_center - self.eye).dot(self.direction());
        let r = radius.max(1e-6) * 1.002 + 1e-6;
        if self.perspective {
            let far = (d + r).max(1e-3);
            let near = (d - r).max(far * 1e-4);
            (near, far.max(near * 1.0001))
        } else {
            (d - r, d + r)
        }
    }

    pub fn projection(&self, vp: Viewport, near: f64, far: f64) -> DMat4 {
        if self.perspective {
            glam::dcamera::rh::proj::directx::perspective(self.fov_y_deg.to_radians(), vp.aspect(), near, far)
        } else {
            let (hx, hy) = self.half_size(vp);
            glam::dcamera::rh::proj::directx::orthographic(-hx, hx, -hy, hy, near, far)
        }
    }

    /// Projection * view with the world shifted by `origin`, for a bounding sphere of the visible content.
    pub fn view_proj(&self, vp: Viewport, origin: DVec3, sphere_center: DVec3, radius: f64) -> DMat4 {
        let (n, f) = self.depth_range(sphere_center, radius);
        self.projection(vp, n, f) * self.view_matrix(origin)
    }

    /// Point on the plane through the centre (perpendicular to the view direction) under a pixel.
    pub fn point_on_center_plane(&self, x: f64, y: f64, vp: Viewport) -> DVec3 {
        let (hx, hy) = self.half_size(vp);
        let nx = 2.0 * x / vp.width - 1.0;
        let ny = 1.0 - 2.0 * y / vp.height;
        self.center + self.right() * (nx * hx) + self.up_vec() * (ny * hy)
    }

    /// Ray through a pixel: origin and unit direction.
    pub fn ray(&self, x: f64, y: f64, vp: Viewport) -> (DVec3, DVec3) {
        let p = self.point_on_center_plane(x, y, vp);
        if self.perspective {
            (self.eye, (p - self.eye).normalize())
        } else {
            let dir = self.direction();
            (p - dir * self.distance().max(1.0), dir)
        }
    }

    /// Orbit about `pivot` (default: the centre): `ang_h` about the screen vertical, `ang_v` about the screen
    /// horizontal, both about the camera axes as they were before the move (`orbit` of the Python viewer).
    pub fn orbit(&mut self, ang_h: f64, ang_v: f64, pivot: Option<DVec3>) {
        let pivot = pivot.unwrap_or(self.center);
        let up = self.up_vec();
        let side = self.direction().cross(up).normalize();
        let rot = DQuat::from_axis_angle(side, ang_v) * DQuat::from_axis_angle(up, ang_h);
        self.transform_about(pivot, rot);
    }

    /// Rotates eye, centre and up by `rot` about `pivot` (`Graphic3d_Camera::Transform` of a rotation).
    pub fn transform_about(&mut self, pivot: DVec3, rot: DQuat) {
        let up = rot * self.up_vec();
        self.eye = pivot + rot * (self.eye - pivot);
        self.center = pivot + rot * (self.center - pivot);
        self.up = up;
        self.orthonormalize();
    }

    /// Rotation about the viewing direction (3D mouse roll keys).
    pub fn roll(&mut self, angle: f64) {
        let rot = DQuat::from_axis_angle(self.direction(), angle);
        self.up = rot * self.up_vec();
        self.orthonormalize();
    }

    /// Pans so that the content follows the cursor: `dx` pixels to the right, `dy` pixels up.
    pub fn pan_pixels(&mut self, dx: f64, dy: f64, vp: Viewport) {
        let k = self.world_per_pixel(vp);
        let t = -(self.right() * dx + self.up_vec() * dy) * k;
        self.eye += t;
        self.center += t;
    }

    /// Zoom by `factor` (> 1 zooms in) keeping the point under the pixel fixed (`_zoom_at`).
    pub fn zoom_at(&mut self, x: f64, y: f64, factor: f64, vp: Viewport) {
        if !(factor.is_finite() && factor > 0.0) {
            return;
        }
        let p = self.point_on_center_plane(x, y, vp);
        let k = 1.0 / factor;
        let nc = p + (self.center - p) * k;
        if self.perspective {
            self.eye = p + (self.eye - p) * k;
            self.center = nc;
        } else {
            let d = nc - self.center;
            self.scale *= k;
            self.center = nc;
            self.eye += d;
        }
    }

    /// 3D mouse in object mode (`space_motion`): `v = (tx, ty, tz, rx, ry, rz)` in the screen frame, rotations about
    /// `pivot`, `dpr` the device pixel ratio.
    pub fn space_motion(&mut self, v: [f64; 6], dt: f64, pivot: DVec3, dpr: f64, vp: Viewport) {
        let [tx, ty, tz, rx, ry, rz] = v;
        if rx != 0.0 || ry != 0.0 || rz != 0.0 {
            let w = self.right() * rx + self.up_vec() * ry + self.direction() * (-rz);
            let mag = w.length();
            if mag > 1e-12 {
                let rot = DQuat::from_axis_angle(w / mag, -mag * SM_ROT * dt);
                self.transform_about(pivot, rot);
            }
        }
        if tx != 0.0 || ty != 0.0 {
            let k = SM_PAN * dpr * dt * self.world_per_pixel(vp);
            let t = self.right() * (-tx * k) + self.up_vec() * (-ty * k);
            self.eye += t;
            self.center += t;
        }
        if tz != 0.0 {
            self.zoom_at(vp.width * 0.5, vp.height * 0.5, (tz * SM_ZOOM * dt).exp(), vp);
        }
    }

    /// Fits a box with `margin` (fraction of the view on each side) around the box centre, keeping the direction.
    pub fn fit_box(&mut self, bounds: &Aabb, margin: f64, vp: Viewport) {
        let lo = DVec3::from_array(bounds.min);
        let hi = DVec3::from_array(bounds.max);
        if !(lo.is_finite() && hi.is_finite()) || lo.x > hi.x || lo.y > hi.y || lo.z > hi.z {
            return;
        }
        let c = (lo + hi) * 0.5;
        let dir = self.direction();
        let right = self.right();
        let up = self.up_vec();
        let k = 1.0 / (1.0 - 2.0 * margin.clamp(0.0, 0.45));
        let corners = (0..8).map(|i| {
            DVec3::new(
                if i & 1 == 0 { lo.x } else { hi.x },
                if i & 2 == 0 { lo.y } else { hi.y },
                if i & 4 == 0 { lo.z } else { hi.z },
            ) - c
        });
        if self.perspective {
            let ty = self.tan_half_fov();
            let tx = ty * vp.aspect();
            let mut d = 1e-6_f64;
            for rel in corners {
                let (x, y, z) = (rel.dot(right).abs(), rel.dot(up).abs(), rel.dot(dir));
                d = d.max(x * k / tx - z).max(y * k / ty - z);
            }
            self.eye = c - dir * d;
            self.center = c;
            self.scale = 2.0 * d * ty;
        } else {
            let (mut hx, mut hy) = (0.0_f64, 0.0_f64);
            for rel in corners {
                hx = hx.max(rel.dot(right).abs());
                hy = hy.max(rel.dot(up).abs());
            }
            let (req_w, req_h) = (2.0 * hx * k, 2.0 * hy * k);
            let aspect = vp.aspect();
            let scale = if aspect >= 1.0 { req_h.max(req_w / aspect) } else { req_w.max(req_h * aspect) };
            let dist = self.distance().max(1e-6);
            self.center = c;
            self.eye = c - dir * dist;
            self.scale = scale.max(1e-9);
        }
    }

    /// `FitAll(0.04)`.
    pub fn fit_all(&mut self, bounds: &Aabb, vp: Viewport) {
        self.fit_box(bounds, FIT_ALL_MARGIN, vp);
    }

    /// Standard view followed by fit-all (`set_orientation`).
    pub fn set_orientation(&mut self, view: StdView, bounds: &Aabb, vp: Viewport) {
        self.set_view(view);
        self.fit_all(bounds, vp);
    }

    /// Projects a world point to physical pixels and NDC depth with a ready `view_proj` (see [`Camera::view_proj`]);
    /// `p` must already be shifted by the same origin. `None` when the point is behind the camera.
    pub fn project_with(view_proj: &DMat4, p: DVec3, vp: Viewport) -> Option<DVec3> {
        let c = *view_proj * DVec4::new(p.x, p.y, p.z, 1.0);
        if c.w <= 1e-12 {
            return None;
        }
        let n = c.truncate() / c.w;
        Some(DVec3::new((n.x * 0.5 + 0.5) * vp.width, (0.5 - n.y * 0.5) * vp.height, n.z))
    }

    /// Inverse of [`Camera::project_with`] for a pixel and NDC depth (result in the same shifted frame).
    pub fn unproject_with(view_proj: &DMat4, x: f64, y: f64, ndc_z: f64, vp: Viewport) -> Option<DVec3> {
        let inv = view_proj.inverse();
        let n = DVec4::new(2.0 * x / vp.width - 1.0, 1.0 - 2.0 * y / vp.height, ndc_z, 1.0);
        let w = inv * n;
        if w.w.abs() < 1e-300 || !w.is_finite() {
            return None;
        }
        Some(w.truncate() / w.w)
    }

    /// Orientation of the camera as a quaternion (columns right, up, -direction).
    pub fn orientation(&self) -> DQuat {
        let r = self.right();
        let u = self.up_vec();
        let b = -self.direction();
        DQuat::from_mat3(&DMat3::from_cols(r, u, b)).normalize()
    }
}

/// Smooth transition between two cameras (view cube click, `SetDuration(0.35)`): orientation by slerp, centre by lerp,
/// distance and scale geometrically.
#[derive(Clone, Copy, Debug)]
pub struct CameraAnimation {
    pub start: Camera,
    pub end: Camera,
    pub duration: f64,
}

impl CameraAnimation {
    pub fn new(start: Camera, end: Camera, duration: f64) -> Self {
        Self { start, end, duration: duration.max(1e-6) }
    }

    pub fn finished(&self, t: f64) -> bool {
        t >= self.duration
    }

    /// Camera `t` seconds after the start.
    pub fn sample(&self, t: f64) -> Camera {
        let f = (t / self.duration).clamp(0.0, 1.0);
        if f >= 1.0 {
            return self.end;
        }
        let q = self.start.orientation().slerp(self.end.orientation(), f);
        let dir = q * DVec3::NEG_Z;
        let up = q * DVec3::Y;
        let center = self.start.center.lerp(self.end.center, f);
        let d0 = self.start.distance().max(1e-9);
        let d1 = self.end.distance().max(1e-9);
        let dist = d0 * (d1 / d0).powf(f);
        let s0 = self.start.scale.max(1e-12);
        let s1 = self.end.scale.max(1e-12);
        let mut cam = self.end;
        cam.center = center;
        cam.eye = center - dir * dist;
        cam.up = up;
        cam.scale = s0 * (s1 / s0).powf(f);
        cam.perspective = self.end.perspective;
        cam
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::{FRAC_PI_2, PI};

    fn vp() -> Viewport {
        Viewport::new(800.0, 600.0)
    }

    fn unit_box() -> Aabb {
        Aabb { min: [-50.0, -20.0, -10.0], max: [50.0, 20.0, 10.0] }
    }

    fn ndc_of(cam: &Camera, p: DVec3, vp: Viewport, bounds: &Aabb) -> DVec3 {
        let c = (DVec3::from_array(bounds.min) + DVec3::from_array(bounds.max)) * 0.5;
        let r = (DVec3::from_array(bounds.max) - DVec3::from_array(bounds.min)).length() * 0.5;
        let m = cam.view_proj(vp, DVec3::ZERO, c, r);
        let s = Camera::project_with(&m, p, vp).expect("in front");
        DVec3::new(s.x / vp.width * 2.0 - 1.0, 1.0 - s.y / vp.height * 2.0, s.z)
    }

    fn corners(b: &Aabb) -> Vec<DVec3> {
        (0..8)
            .map(|i| {
                DVec3::new(
                    if i & 1 == 0 { b.min[0] } else { b.max[0] },
                    if i & 2 == 0 { b.min[1] } else { b.max[1] },
                    if i & 4 == 0 { b.min[2] } else { b.max[2] },
                )
            })
            .collect()
    }

    #[test]
    fn std_view_directions_follow_occt_zup() {
        assert_eq!(StdView::Front.eye_direction(), DVec3::NEG_Y);
        assert_eq!(StdView::Back.eye_direction(), DVec3::Y);
        assert_eq!(StdView::Top.eye_direction(), DVec3::Z);
        assert_eq!(StdView::Bottom.eye_direction(), DVec3::NEG_Z);
        assert_eq!(StdView::Left.eye_direction(), DVec3::NEG_X);
        assert_eq!(StdView::Right.eye_direction(), DVec3::X);
        let iso = StdView::Iso.eye_direction();
        assert!((iso - DVec3::new(1.0, -1.0, 1.0) / 3f64.sqrt()).length() < 1e-12);
        assert_eq!(StdView::Top.up(), DVec3::Y);
        assert_eq!(StdView::Front.up(), DVec3::Z);
        for v in StdView::ALL {
            assert_eq!(StdView::from_key(v.key()), Some(v));
        }
        assert_eq!(StdView::from_key("nope"), None);
    }

    #[test]
    fn front_view_has_x_to_the_right_and_z_up() {
        let mut c = Camera::default();
        c.set_view(StdView::Front);
        assert!((c.right() - DVec3::X).length() < 1e-12);
        assert!((c.up_vec() - DVec3::Z).length() < 1e-12);
        c.set_view(StdView::Right);
        assert!((c.right() - DVec3::Y).length() < 1e-12);
        c.set_view(StdView::Top);
        assert!((c.right() - DVec3::X).length() < 1e-12);
        assert!((c.up_vec() - DVec3::Y).length() < 1e-12);
    }

    #[test]
    fn centre_projects_to_screen_centre_and_roundtrips() {
        let b = unit_box();
        for persp in [false, true] {
            let mut c = Camera::default();
            c.set_view(StdView::Iso);
            c.set_perspective(persp, vp());
            c.fit_all(&b, vp());
            let ctr = DVec3::from_array(b.min).midpoint(DVec3::from_array(b.max));
            let n = ndc_of(&c, ctr, vp(), &b);
            assert!(n.x.abs() < 1e-9 && n.y.abs() < 1e-9, "{n:?}");
            let r = (DVec3::from_array(b.max) - DVec3::from_array(b.min)).length() * 0.5;
            let m = c.view_proj(vp(), DVec3::ZERO, ctr, r);
            let p = DVec3::new(12.0, -7.0, 3.0);
            let s = Camera::project_with(&m, p, vp()).expect("projects");
            let back = Camera::unproject_with(&m, s.x, s.y, s.z, vp()).expect("unprojects");
            assert!((back - p).length() < 1e-6, "{back:?} vs {p:?}");
        }
    }

    #[test]
    fn fit_all_uses_the_requested_margin_ortho_and_perspective() {
        let b = unit_box();
        for (w, h) in [(800.0, 600.0), (400.0, 900.0), (500.0, 500.0)] {
            let v = Viewport::new(w, h);
            for persp in [false, true] {
                for view in [StdView::Iso, StdView::Front, StdView::Top] {
                    let mut c = Camera::default();
                    c.set_view(view);
                    c.set_perspective(persp, v);
                    c.fit_all(&b, v);
                    let mut mx = 0.0_f64;
                    for p in corners(&b) {
                        let n = ndc_of(&c, p, v, &b);
                        mx = mx.max(n.x.abs()).max(n.y.abs());
                        assert!(n.z > 0.0 && n.z < 1.0, "depth {n:?}");
                    }
                    let want = 1.0 - 2.0 * FIT_ALL_MARGIN;
                    assert!(mx <= want + 1e-6, "{persp} {view:?} {w}x{h}: {mx} > {want}");
                    if !persp {
                        assert!((mx - want).abs() < 1e-6, "tight fit expected: {mx}");
                    }
                }
            }
        }
    }

    #[test]
    fn perspective_fit_is_tight_on_the_limiting_axis() {
        let b = unit_box();
        let v = vp();
        let mut c = Camera::default();
        c.set_view(StdView::Front);
        c.set_perspective(true, v);
        c.fit_box(&b, FIT_SELECTION_MARGIN, v);
        let mut mx = 0.0_f64;
        for p in corners(&b) {
            let n = ndc_of(&c, p, v, &b);
            mx = mx.max(n.x.abs()).max(n.y.abs());
        }
        assert!((mx - (1.0 - 2.0 * FIT_SELECTION_MARGIN)).abs() < 1e-6, "{mx}");
    }

    #[test]
    fn fit_ignores_void_boxes() {
        let mut c = Camera::default();
        let before = c;
        c.fit_box(&Aabb { min: [1.0; 3], max: [-1.0; 3] }, 0.04, vp());
        assert_eq!(c, before);
        c.fit_box(&Aabb { min: [f64::NAN; 3], max: [1.0; 3] }, 0.04, vp());
        assert_eq!(c, before);
    }

    #[test]
    fn orbit_quarter_turn_about_up_moves_front_to_right() {
        let mut c = Camera::default();
        c.set_view(StdView::Front);
        c.orbit(FRAC_PI_2, 0.0, None);
        // eye was on -Y, a counter-clockwise turn about +Z carries it to +X
        let d = (c.eye - c.center).normalize();
        assert!((d - DVec3::X).length() < 1e-12, "{d:?}");
        assert!((c.up_vec() - DVec3::Z).length() < 1e-12);
    }

    #[test]
    fn orbit_vertical_tilts_towards_top() {
        let mut c = Camera::default();
        c.set_view(StdView::Front);
        c.orbit(0.0, -FRAC_PI_2, None);
        let d = (c.eye - c.center).normalize();
        assert!((d - DVec3::Z).length() < 1e-12, "{d:?}");
    }

    #[test]
    fn orbit_full_turn_returns() {
        let mut c = Camera::default();
        let start = c;
        for _ in 0..24 {
            c.orbit(PI / 12.0, 0.0, Some(DVec3::new(5.0, 1.0, 2.0)));
        }
        assert!((c.eye - start.eye).length() < 1e-9);
        assert!((c.up_vec() - start.up_vec()).length() < 1e-9);
    }

    #[test]
    fn orbit_keeps_the_pivot_on_screen() {
        let b = unit_box();
        let pivot = DVec3::new(20.0, 5.0, -3.0);
        for persp in [false, true] {
            let mut c = Camera::default();
            c.set_perspective(persp, vp());
            c.fit_all(&b, vp());
            let before = ndc_of(&c, pivot, vp(), &b);
            c.orbit(0.37, -0.21, Some(pivot));
            let after = ndc_of(&c, pivot, vp(), &b);
            assert!((before.x - after.x).abs() < 1e-9 && (before.y - after.y).abs() < 1e-9, "{before:?} {after:?}");
        }
    }

    #[test]
    fn zoom_at_keeps_the_point_under_the_cursor() {
        let b = unit_box();
        for persp in [false, true] {
            let mut c = Camera::default();
            c.set_view(StdView::Iso);
            c.set_perspective(persp, vp());
            c.fit_all(&b, vp());
            let (cx, cy) = (600.0, 150.0);
            let before = c.point_on_center_plane(cx, cy, vp());
            c.zoom_at(cx, cy, 1.2, vp());
            let after = c.point_on_center_plane(cx, cy, vp());
            assert!((before - after).length() < 1e-9, "persp={persp}: {before:?} {after:?}");
            // 1.2 times closer / larger
            if !persp {
                let c2 = {
                    let mut k = Camera::default();
                    k.set_view(StdView::Iso);
                    k.fit_all(&b, vp());
                    k
                };
                assert!((c.scale * 1.2 - c2.scale).abs() < 1e-9);
            }
        }
    }

    #[test]
    fn zoom_in_then_out_restores() {
        let mut c = Camera::default();
        c.fit_all(&unit_box(), vp());
        let start = c;
        c.zoom_at(100.0, 400.0, 1.2, vp());
        c.zoom_at(100.0, 400.0, 1.0 / 1.2, vp());
        assert!((c.scale - start.scale).abs() < 1e-9);
        assert!((c.center - start.center).length() < 1e-9);
        // nonsense factors are ignored
        let before = c;
        c.zoom_at(1.0, 1.0, 0.0, vp());
        c.zoom_at(1.0, 1.0, f64::NAN, vp());
        assert_eq!(c, before);
    }

    #[test]
    fn pan_moves_content_with_the_cursor() {
        let b = unit_box();
        let ctr = DVec3::ZERO;
        let mut c = Camera::default();
        c.fit_all(&b, vp());
        let before = ndc_of(&c, ctr, vp(), &b);
        c.pan_pixels(40.0, -25.0, vp());
        let after = ndc_of(&c, ctr, vp(), &b);
        // 40 px right = 40 / 400 in ndc; 25 px down = -25 / 300
        assert!((after.x - before.x - 40.0 / 400.0).abs() < 1e-9, "{after:?}");
        assert!((after.y - before.y + 25.0 / 300.0).abs() < 1e-9, "{after:?}");
    }

    #[test]
    fn switching_projection_keeps_apparent_size() {
        for (w, h) in [(800.0, 600.0), (300.0, 700.0)] {
            let v = Viewport::new(w, h);
            let mut c = Camera::default();
            c.fit_all(&unit_box(), v);
            let wpp = c.world_per_pixel(v);
            c.set_perspective(true, v);
            assert!((c.world_per_pixel(v) - wpp).abs() < 1e-9 * wpp.max(1.0));
            c.set_perspective(false, v);
            assert!((c.world_per_pixel(v) - wpp).abs() < 1e-9 * wpp.max(1.0));
        }
    }

    #[test]
    fn world_per_pixel_matches_scale() {
        let v = Viewport::new(800.0, 600.0);
        let mut c = Camera::default();
        c.scale = 600.0;
        assert!((c.world_per_pixel(v) - 1.0).abs() < 1e-12);
        let tall = Viewport::new(300.0, 900.0);
        c.scale = 300.0;
        assert!((c.world_per_pixel(tall) - 1.0).abs() < 1e-12);
        assert!((c.ortho_units_per_pixel(tall) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn depth_range_encloses_the_sphere() {
        let mut c = Camera::default();
        c.set_view(StdView::Front);
        c.eye = c.center + DVec3::NEG_Y * 500.0;
        let (n, f) = c.depth_range(DVec3::ZERO, 100.0);
        assert!(n < 400.0 && f > 600.0);
        c.set_perspective(true, vp());
        c.eye = c.center + DVec3::NEG_Y * 500.0;
        let (n, f) = c.depth_range(DVec3::ZERO, 100.0);
        assert!(n > 0.0 && n < 400.0 && f > 600.0 && f > n);
        // camera inside the sphere: near stays positive
        let (n, f) = c.depth_range(DVec3::ZERO, 1000.0);
        assert!(n > 0.0 && f > n);
    }

    #[test]
    fn ray_through_the_centre_pixel_hits_the_centre() {
        for persp in [false, true] {
            let mut c = Camera::default();
            c.set_perspective(persp, vp());
            c.fit_all(&unit_box(), vp());
            let (o, d) = c.ray(400.0, 300.0, vp());
            let t = (c.center - o).dot(d);
            assert!((o + d * t - c.center).length() < 1e-9);
            assert!((d - c.direction()).length() < 1e-9);
        }
    }

    #[test]
    fn roll_rotates_up_about_the_direction() {
        let mut c = Camera::default();
        c.set_view(StdView::Front);
        c.roll(FRAC_PI_2);
        assert!((c.up_vec() - DVec3::X).length() < 1e-12 || (c.up_vec() + DVec3::X).length() < 1e-12);
        let dir = c.direction();
        assert!(c.up_vec().dot(dir).abs() < 1e-12);
    }

    #[test]
    fn space_motion_zoom_and_pan() {
        let v = vp();
        let mut c = Camera::default();
        c.fit_all(&unit_box(), v);
        let s0 = c.scale;
        c.space_motion([0.0, 0.0, 1.0, 0.0, 0.0, 0.0], 0.1, DVec3::ZERO, 1.0, v);
        assert!((c.scale - s0 / (SM_ZOOM * 0.1).exp()).abs() < 1e-9);
        let before = c.center;
        let wpp = c.world_per_pixel(v);
        c.space_motion([1.0, 0.0, 0.0, 0.0, 0.0, 0.0], 0.1, DVec3::ZERO, 1.0, v);
        // object mode: the model follows the cap, the camera moves opposite (-right)
        let moved = (c.center - before).dot(c.right());
        assert!((moved + SM_PAN * 0.1 * wpp).abs() < 1e-9, "{moved}");
        // rotation about the pivot keeps the pivot fixed on screen
        let pivot = DVec3::new(3.0, 4.0, 5.0);
        let b = unit_box();
        let p0 = ndc_of(&c, pivot, v, &b);
        c.space_motion([0.0, 0.0, 0.0, 0.0, 1.0, 0.0], 0.1, pivot, 1.0, v);
        let p1 = ndc_of(&c, pivot, v, &b);
        assert!((p0.x - p1.x).abs() < 1e-9 && (p0.y - p1.y).abs() < 1e-9);
    }

    #[test]
    fn animation_hits_both_ends_and_stays_orthonormal() {
        let b = unit_box();
        let mut a = Camera::default();
        a.set_view(StdView::Front);
        a.fit_all(&b, vp());
        let mut e = a;
        e.set_orientation(StdView::Right, &Aabb { min: [-5.0; 3], max: [5.0; 3] }, vp());
        let anim = CameraAnimation::new(a, e, 0.35);
        let s0 = anim.sample(0.0);
        assert!((s0.eye - a.eye).length() < 1e-6 && (s0.center - a.center).length() < 1e-6);
        assert_eq!(anim.sample(1.0), e);
        assert!(anim.finished(0.35) && !anim.finished(0.1));
        let m = anim.sample(0.175);
        assert!((m.right().length() - 1.0).abs() < 1e-9);
        assert!(m.up_vec().dot(m.direction()).abs() < 1e-9);
        assert!(m.scale < a.scale && m.scale > e.scale);
    }

    #[test]
    fn up_for_direction_switches_on_the_z_axis() {
        assert_eq!(up_for_direction(DVec3::Z), DVec3::Y);
        assert_eq!(up_for_direction(DVec3::NEG_Z), DVec3::Y);
        assert_eq!(up_for_direction(DVec3::new(1.0, 1.0, 1.0)), DVec3::Z);
    }
}
