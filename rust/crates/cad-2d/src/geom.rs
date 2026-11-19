//! Small 3D / 2D geometry toolbox: vectors, affine transforms, OCS, arcs, Bezier curves, paths and path elements.
use std::f64::consts::{PI, TAU};

pub type V3 = [f64; 3];
pub type V2 = [f64; 2];

pub const NULL3: V3 = [0.0; 3];
pub const X_AXIS: V3 = [1.0, 0.0, 0.0];
pub const Y_AXIS: V3 = [0.0, 1.0, 0.0];
pub const Z_AXIS: V3 = [0.0, 0.0, 1.0];

pub fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
pub fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
pub fn mul(a: V3, s: f64) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
pub fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
pub fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
pub fn len(a: V3) -> f64 {
    dot(a, a).sqrt()
}
pub fn dist(a: V3, b: V3) -> f64 {
    len(sub(a, b))
}
pub fn normalize(a: V3) -> V3 {
    let l = len(a);
    if l == 0.0 {
        a
    } else {
        mul(a, 1.0 / l)
    }
}
pub fn lerp(a: V3, b: V3, t: f64) -> V3 {
    add(a, mul(sub(b, a), t))
}
pub fn lerp2(a: V2, b: V2, t: f64) -> V2 {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
}

/// `math.isclose(a, b, rel_tol, abs_tol)`.
pub fn isclose(a: f64, b: f64, rel: f64, abs: f64) -> bool {
    if a == b {
        return true;
    }
    (a - b).abs() <= (rel * a.abs().max(b.abs())).max(abs)
}

/// ezdxf `Vec3.isclose` (rel_tol 1e-9, abs_tol 1e-12 per component).
pub fn v3_close(a: V3, b: V3) -> bool {
    v3_close_tol(a, b, 1e-9, 1e-12)
}

pub fn v3_close_tol(a: V3, b: V3, rel: f64, abs: f64) -> bool {
    (0..3).all(|i| isclose(a[i], b[i], rel, abs))
}

pub fn v2_close(a: V2, b: V2) -> bool {
    isclose(a[0], b[0], 1e-9, 1e-12) && isclose(a[1], b[1], 1e-9, 1e-12)
}

pub fn is_null(a: V3) -> bool {
    a[0].abs() < 1e-12 && a[1].abs() < 1e-12 && a[2].abs() < 1e-12
}

/// Python `%` for floats (result has the sign of the divisor).
pub fn fmod(a: f64, b: f64) -> f64 {
    let r = a % b;
    if r != 0.0 && ((r < 0.0) != (b < 0.0)) {
        r + b
    } else {
        r
    }
}

// ------------------------------------------------------------------------------------------------ transforms
/// Affine transformation `p' = M p + t` (column vectors).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Xform {
    pub m: [[f64; 3]; 3],
    pub t: V3,
}

impl Xform {
    pub const IDENTITY: Xform = Xform { m: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], t: [0.0; 3] };

    pub fn translate(t: V3) -> Xform {
        Xform { t, ..Self::IDENTITY }
    }

    pub fn scale(s: V3) -> Xform {
        Xform { m: [[s[0], 0.0, 0.0], [0.0, s[1], 0.0], [0.0, 0.0, s[2]]], t: [0.0; 3] }
    }

    /// Rotation about the z axis by `a` radians.
    pub fn rot_z(a: f64) -> Xform {
        let (s, c) = a.sin_cos();
        Xform { m: [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]], t: [0.0; 3] }
    }

    /// Rotation about an arbitrary axis (Rodrigues).
    pub fn axis_rotate(axis: V3, a: f64) -> Xform {
        let u = normalize(axis);
        let (s, c) = a.sin_cos();
        let (x, y, z) = (u[0], u[1], u[2]);
        let k = 1.0 - c;
        Xform {
            m: [
                [c + x * x * k, x * y * k - z * s, x * z * k + y * s],
                [y * x * k + z * s, c + y * y * k, y * z * k - x * s],
                [z * x * k - y * s, z * y * k + x * s, c + z * z * k],
            ],
            t: [0.0; 3],
        }
    }

    /// Matrix that maps the local axes `ux`, `uy`, `uz` (columns) and origin to world space.
    pub fn from_axes(ux: V3, uy: V3, uz: V3, origin: V3) -> Xform {
        Xform { m: [[ux[0], uy[0], uz[0]], [ux[1], uy[1], uz[1]], [ux[2], uy[2], uz[2]]], t: origin }
    }

    pub fn apply(&self, p: V3) -> V3 {
        let m = &self.m;
        [
            m[0][0] * p[0] + m[0][1] * p[1] + m[0][2] * p[2] + self.t[0],
            m[1][0] * p[0] + m[1][1] * p[1] + m[1][2] * p[2] + self.t[1],
            m[2][0] * p[0] + m[2][1] * p[1] + m[2][2] * p[2] + self.t[2],
        ]
    }

    pub fn apply_dir(&self, p: V3) -> V3 {
        let m = &self.m;
        [
            m[0][0] * p[0] + m[0][1] * p[1] + m[0][2] * p[2],
            m[1][0] * p[0] + m[1][1] * p[1] + m[1][2] * p[2],
            m[2][0] * p[0] + m[2][1] * p[1] + m[2][2] * p[2],
        ]
    }

    pub fn apply2(&self, p: V2) -> V2 {
        let q = self.apply([p[0], p[1], 0.0]);
        [q[0], q[1]]
    }

    /// `self` first, then `next`.
    pub fn then(&self, next: &Xform) -> Xform {
        let mut m = [[0.0; 3]; 3];
        for (i, row) in m.iter_mut().enumerate() {
            for (j, v) in row.iter_mut().enumerate() {
                *v = (0..3).map(|k| next.m[i][k] * self.m[k][j]).sum();
            }
        }
        Xform { m, t: next.apply(self.t) }
    }

    pub fn is_identity(&self) -> bool {
        *self == Self::IDENTITY
    }

    /// Determinant of the linear part.
    pub fn det(&self) -> f64 {
        let m = &self.m;
        m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
    }

    /// Uniform scale factor of the xy plane when the matrix is a similarity there, else None.
    pub fn uniform_scale_2d(&self) -> Option<f64> {
        let a = [self.m[0][0], self.m[1][0]];
        let b = [self.m[0][1], self.m[1][1]];
        let la = (a[0] * a[0] + a[1] * a[1]).sqrt();
        let lb = (b[0] * b[0] + b[1] * b[1]).sqrt();
        let d = a[0] * b[0] + a[1] * b[1];
        if (la - lb).abs() <= 1e-9 * la.max(lb).max(1e-300) && d.abs() <= 1e-9 * la * lb {
            Some(la)
        } else {
            None
        }
    }
}

/// Object coordinate system defined by an extrusion vector (arbitrary axis algorithm).
#[derive(Clone, Copy, Debug)]
pub struct Ocs {
    pub ux: V3,
    pub uy: V3,
    pub uz: V3,
    pub transform: bool,
}

impl Ocs {
    pub fn new(extrusion: V3) -> Ocs {
        let az = normalize(extrusion);
        if is_null(extrusion) || v3_close_tol(az, Z_AXIS, 1e-9, 1e-12) {
            return Ocs { ux: X_AXIS, uy: Y_AXIS, uz: Z_AXIS, transform: false };
        }
        let ax = if az[0].abs() < 1.0 / 64.0 && az[1].abs() < 1.0 / 64.0 { cross(Y_AXIS, az) } else { cross(Z_AXIS, az) };
        let ux = normalize(ax);
        let uy = normalize(cross(az, ux));
        Ocs { ux, uy, uz: az, transform: true }
    }

    pub fn to_wcs(&self, p: V3) -> V3 {
        if !self.transform {
            return p;
        }
        add(add(mul(self.ux, p[0]), mul(self.uy, p[1])), mul(self.uz, p[2]))
    }

    pub fn from_wcs(&self, p: V3) -> V3 {
        if !self.transform {
            return p;
        }
        [dot(p, self.ux), dot(p, self.uy), dot(p, self.uz)]
    }

    pub fn xform(&self) -> Xform {
        Xform::from_axes(self.ux, self.uy, self.uz, NULL3)
    }
}

// ------------------------------------------------------------------------------------------------ angles and arcs
pub const RAD_ABS_TOL: f64 = 1e-9;
pub const DEG_ABS_TOL: f64 = 1e-9;

/// Counter-clockwise span `start` -> `end` in degrees, 360 for a full circle (ezdxf `arc_angle_span_deg`).
pub fn arc_span_deg(start: f64, end: f64) -> f64 {
    if isclose(start, end, 1e-9, DEG_ABS_TOL) {
        return 0.0;
    }
    let s = fmod(start, 360.0);
    if isclose(s, fmod(end, 360.0), 1e-9, DEG_ABS_TOL) {
        return 360.0;
    }
    let mut e = end;
    if !isclose(end, 360.0, 1e-9, DEG_ABS_TOL) {
        e = fmod(end, 360.0);
    }
    if e < s {
        e += 360.0;
    }
    e - s
}

/// Counter-clockwise span in radians, 2 pi for a full circle (ezdxf `arc_angle_span_rad`).
pub fn arc_span_rad(start: f64, end: f64) -> f64 {
    if isclose(start, end, 1e-9, RAD_ABS_TOL) {
        return 0.0;
    }
    let s = fmod(start, TAU);
    if isclose(s, fmod(end, TAU), 1e-9, RAD_ABS_TOL) {
        return TAU;
    }
    let mut e = end;
    if !isclose(end, TAU, 1e-9, RAD_ABS_TOL) {
        e = fmod(end, TAU);
    }
    if e < s {
        e += TAU;
    }
    e - s
}

/// Arc parameters of a bulge segment: (center, start angle, end angle, radius), counter-clockwise (ezdxf `bulge_to_arc`).
pub fn bulge_to_arc(p1: V2, p2: V2, bulge: f64) -> (V2, f64, f64, f64) {
    let d = ((p2[0] - p1[0]).powi(2) + (p2[1] - p1[1]).powi(2)).sqrt();
    let r = d * (1.0 + bulge * bulge) / 4.0 / bulge;
    let a = (p2[1] - p1[1]).atan2(p2[0] - p1[0]) + (PI / 2.0 - bulge.atan() * 2.0);
    let c = [p1[0] + r * a.cos(), p1[1] + r * a.sin()];
    let ang = |from: V2, to: V2| (to[1] - from[1]).atan2(to[0] - from[0]);
    if bulge < 0.0 {
        (c, ang(c, p2), ang(c, p1), r.abs())
    } else {
        (c, ang(c, p1), ang(c, p2), r.abs())
    }
}

// ------------------------------------------------------------------------------------------------ Bezier curves
/// Cubic Bezier segments approximating the arc `a0 -> a1` (radians, a1 > a0) of the unit circle (ezdxf `cubic_bezier_arc_parameters`).
pub fn unit_arc_beziers(a0: f64, a1: f64, segments: usize) -> Vec<[V2; 4]> {
    let delta = a1 - a0;
    if delta <= 0.0 {
        return Vec::new();
    }
    let n = ((delta / PI * 2.0).ceil() as usize).max(segments).max(1);
    let seg = delta / n as f64;
    let tl = (4.0 / 3.0) * (seg / 4.0).tan();
    let mut out = Vec::with_capacity(n);
    let mut ang = a0;
    let mut end = [ang.cos(), ang.sin()];
    for _ in 0..n {
        let start = end;
        ang += seg;
        end = [ang.cos(), ang.sin()];
        let c1 = [start[0] - start[1] * tl, start[1] + start[0] * tl];
        let c2 = [end[0] + end[1] * tl, end[1] - end[0] * tl];
        out.push([start, c1, c2, end]);
    }
    out
}

/// Elliptical arc as cubic Bezier segments in 3D: `center + major * x + minor * y` (ezdxf `cubic_bezier_from_ellipse`).
pub fn ellipse_beziers(center: V3, major: V3, minor: V3, start_param: f64, end_param: f64, segments: usize) -> Vec<[V3; 4]> {
    let span = arc_span_rad(start_param, end_param);
    if span.abs() < 1e-9 {
        return Vec::new();
    }
    let a0 = fmod(start_param, TAU);
    let a1 = a0 + span;
    let map = |p: V2| add(center, add(mul(major, p[0]), mul(minor, p[1])));
    unit_arc_beziers(a0, a1, segments).into_iter().map(|c| [map(c[0]), map(c[1]), map(c[2]), map(c[3])]).collect()
}

/// Minor axis of an ellipse (ezdxf `minor_axis`).
pub fn minor_axis(major: V3, extrusion: V3, ratio: f64) -> V3 {
    let c = cross(extrusion, major);
    let l = len(c);
    if l == 0.0 {
        return NULL3;
    }
    mul(c, len(major) * ratio / l)
}

pub fn cubic_point(p: &[V3; 4], t: f64) -> V3 {
    let u = 1.0 - t;
    let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
    [
        p[0][0] * a + p[1][0] * b + p[2][0] * c + p[3][0] * d,
        p[0][1] * a + p[1][1] * b + p[2][1] * c + p[3][1] * d,
        p[0][2] * a + p[1][2] * b + p[2][2] * c + p[3][2] * d,
    ]
}

pub fn cubic_point2(p: &[V2; 4], t: f64) -> V2 {
    let u = 1.0 - t;
    let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
    [p[0][0] * a + p[1][0] * b + p[2][0] * c + p[3][0] * d, p[0][1] * a + p[1][1] * b + p[2][1] * c + p[3][1] * d]
}

fn dist_point_line2(p: V2, a: V2, b: V2) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l = (dx * dx + dy * dy).sqrt();
    if l < 1e-300 {
        return ((p[0] - a[0]).powi(2) + (p[1] - a[1]).powi(2)).sqrt();
    }
    ((p[0] - a[0]) * dy - (p[1] - a[1]) * dx).abs() / l
}

/// Appends the flattened cubic (without its start point) to `out`: `segments` parameter intervals at least, each
/// subdivided until the curve midpoint is closer than `distance` to the chord.
pub fn flatten_cubic(p: &[V2; 4], distance: f64, segments: usize, out: &mut Vec<V2>) {
    let n = segments.max(1);
    for i in 0..n {
        let t0 = i as f64 / n as f64;
        let t1 = (i + 1) as f64 / n as f64;
        let a = if i == 0 { p[0] } else { cubic_point2(p, t0) };
        let b = if i + 1 == n { p[3] } else { cubic_point2(p, t1) };
        subdivide(p, t0, t1, a, b, distance, 0, out);
    }
}

#[allow(clippy::too_many_arguments)]
fn subdivide(p: &[V2; 4], t0: f64, t1: f64, a: V2, b: V2, distance: f64, depth: u32, out: &mut Vec<V2>) {
    let tm = 0.5 * (t0 + t1);
    let m = cubic_point2(p, tm);
    if depth >= 14 || dist_point_line2(m, a, b) <= distance {
        out.push(b);
    } else {
        subdivide(p, t0, tm, a, m, distance, depth + 1, out);
        subdivide(p, tm, t1, m, b, distance, depth + 1, out);
    }
}

// ------------------------------------------------------------------------------------------------ Qt style path elements
pub const M_MOVE: u8 = 0;
pub const M_LINE: u8 = 1;
pub const M_CURVE: u8 = 2;
pub const M_CDATA: u8 = 3;

/// Path in the element form of QPainterPath: MoveTo / LineTo / (CurveTo, CurveToData, CurveToData).
#[derive(Clone, Debug, Default)]
pub struct Elems {
    pub ty: Vec<u8>,
    pub xy: Vec<V2>,
}

/// Polylines of the subpaths of a path given as element types and points (`Elems::flatten` without a copy of the path); curves
/// are flattened with `distance` / `segments` (see `flatten_cubic`).
pub fn flatten_path(ty: &[u8], xy: &[V2], distance: f64, segments: usize) -> Vec<Vec<V2>> {
    let mut subs: Vec<Vec<V2>> = Vec::new();
    let mut cur: Vec<V2> = Vec::new();
    let mut i = 0;
    while i < ty.len() {
        match ty[i] {
            M_MOVE => {
                if cur.len() > 1 {
                    subs.push(std::mem::take(&mut cur));
                } else {
                    cur.clear();
                }
                cur.push(xy[i]);
                i += 1;
            }
            M_LINE => {
                cur.push(xy[i]);
                i += 1;
            }
            M_CURVE if i + 2 < ty.len() => {
                let start = cur.last().copied().unwrap_or(xy[i]);
                if cur.is_empty() {
                    cur.push(start);
                }
                flatten_cubic(&[start, xy[i], xy[i + 1], xy[i + 2]], distance, segments, &mut cur);
                i += 3;
            }
            _ => i += 1,
        }
    }
    if cur.len() > 1 {
        subs.push(cur);
    }
    subs
}

impl Elems {
    pub fn new() -> Elems {
        Elems::default()
    }

    pub fn len(&self) -> usize {
        self.ty.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ty.is_empty()
    }

    pub fn move_to(&mut self, p: V2) {
        self.ty.push(M_MOVE);
        self.xy.push(p);
    }

    pub fn line_to(&mut self, p: V2) {
        self.ty.push(M_LINE);
        self.xy.push(p);
    }

    pub fn curve_to(&mut self, c1: V2, c2: V2, end: V2) {
        self.ty.extend_from_slice(&[M_CURVE, M_CDATA, M_CDATA]);
        self.xy.extend_from_slice(&[c1, c2, end]);
    }

    /// Quadratic curve as the equivalent cubic (same as `QPainterPath::quadTo`).
    pub fn quad_to(&mut self, ctrl: V2, end: V2) {
        let p0 = self.xy.last().copied().unwrap_or(end);
        let c1 = [p0[0] + (ctrl[0] - p0[0]) * 2.0 / 3.0, p0[1] + (ctrl[1] - p0[1]) * 2.0 / 3.0];
        let c2 = [end[0] + (ctrl[0] - end[0]) * 2.0 / 3.0, end[1] + (ctrl[1] - end[1]) * 2.0 / 3.0];
        self.curve_to(c1, c2, end);
    }

    pub fn extend(&mut self, other: &Elems) {
        self.ty.extend_from_slice(&other.ty);
        self.xy.extend_from_slice(&other.xy);
    }

    pub fn transform(&mut self, x: &Xform) {
        for p in &mut self.xy {
            *p = x.apply2(*p);
        }
    }

    /// Polylines of the subpaths; curves are flattened with `distance` / `segments` (see `flatten_cubic`).
    pub fn flatten(&self, distance: f64, segments: usize) -> Vec<Vec<V2>> {
        flatten_path(&self.ty, &self.xy, distance, segments)
    }

    /// Axis aligned bounds of all elements including curve control points (what the batching scene builder used).
    pub fn bounds(&self) -> Option<[f64; 4]> {
        let mut it = self.xy.iter();
        let f = it.next()?;
        let mut b = [f[0], f[1], f[0], f[1]];
        for p in it {
            b[0] = b[0].min(p[0]);
            b[1] = b[1].min(p[1]);
            b[2] = b[2].max(p[0]);
            b[3] = b[3].max(p[1]);
        }
        Some(b)
    }
}

// ------------------------------------------------------------------------------------------------ 3D path
#[derive(Clone, Copy, Debug)]
pub enum Cmd {
    Line(V3),
    Move(V3),
    Curve3 { end: V3, ctrl: V3 },
    Curve4 { end: V3, c1: V3, c2: V3 },
}

impl Cmd {
    pub fn end(&self) -> V3 {
        match *self {
            Cmd::Line(e) | Cmd::Move(e) => e,
            Cmd::Curve3 { end, .. } | Cmd::Curve4 { end, .. } => end,
        }
    }
}

/// ezdxf `Path`: a start point and a command list; MOVE commands start sub paths.
#[derive(Clone, Debug, Default)]
pub struct Path {
    pub start: V3,
    pub cmds: Vec<Cmd>,
}

impl Path {
    pub fn new(start: V3) -> Path {
        Path { start, cmds: Vec::new() }
    }

    pub fn len(&self) -> usize {
        self.cmds.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cmds.is_empty()
    }

    pub fn end(&self) -> V3 {
        self.cmds.last().map(|c| c.end()).unwrap_or(self.start)
    }

    pub fn line_to(&mut self, p: V3) {
        self.cmds.push(Cmd::Line(p));
    }

    pub fn move_to(&mut self, p: V3) {
        self.cmds.push(Cmd::Move(p));
    }

    pub fn curve4_to(&mut self, end: V3, c1: V3, c2: V3) {
        self.cmds.push(Cmd::Curve4 { end, c1, c2 });
    }

    pub fn curve3_to(&mut self, end: V3, ctrl: V3) {
        self.cmds.push(Cmd::Curve3 { end, ctrl });
    }

    pub fn is_closed(&self) -> bool {
        v3_close(self.start, self.end())
    }

    pub fn close(&mut self) {
        if !self.is_closed() {
            self.line_to(self.start);
        }
    }

    /// Start of the last sub path.
    fn sub_start(&self) -> V3 {
        for c in self.cmds.iter().rev() {
            if let Cmd::Move(p) = c {
                return *p;
            }
        }
        self.start
    }

    /// Closes the current sub path.
    pub fn close_sub_path(&mut self) {
        let s = self.sub_start();
        if !v3_close(s, self.end()) {
            self.line_to(s);
        }
    }

    pub fn has_sub_paths(&self) -> bool {
        self.cmds.iter().any(|c| matches!(c, Cmd::Move(_)))
    }

    pub fn sub_paths(&self) -> Vec<Path> {
        let mut out = Vec::new();
        let mut cur = Path::new(self.start);
        for c in &self.cmds {
            if let Cmd::Move(p) = c {
                out.push(std::mem::replace(&mut cur, Path::new(*p)));
            } else {
                cur.cmds.push(*c);
            }
        }
        out.push(cur);
        out
    }

    pub fn reversed(&self) -> Path {
        // sub paths are reversed in place keeping their order
        let mut out = Path::default();
        let mut first = true;
        for sp in self.sub_paths() {
            let r = reverse_simple(&sp);
            if first {
                out = r;
                first = false;
            } else {
                out.move_to(r.start);
                out.cmds.extend(r.cmds);
            }
        }
        out
    }

    pub fn append_path(&mut self, other: &Path) {
        if self.cmds.is_empty() && v3_close(self.start, NULL3) && false {
            return;
        }
        if !v3_close(self.end(), other.start) {
            self.line_to(other.start);
        }
        self.cmds.extend_from_slice(&other.cmds);
    }

    pub fn transformed(&self, x: &Xform) -> Path {
        let mut p = Path::new(x.apply(self.start));
        for c in &self.cmds {
            p.cmds.push(match *c {
                Cmd::Line(e) => Cmd::Line(x.apply(e)),
                Cmd::Move(e) => Cmd::Move(x.apply(e)),
                Cmd::Curve3 { end, ctrl } => Cmd::Curve3 { end: x.apply(end), ctrl: x.apply(ctrl) },
                Cmd::Curve4 { end, c1, c2 } => Cmd::Curve4 { end: x.apply(end), c1: x.apply(c1), c2: x.apply(c2) },
            });
        }
        p
    }

    /// Qt style elements of the xy projection after `x`.
    pub fn to_elems(&self, x: &Xform) -> Elems {
        let mut e = Elems::new();
        e.move_to(x.apply2([self.start[0], self.start[1]]));
        for c in &self.cmds {
            match *c {
                Cmd::Line(p) => e.line_to(x.apply2([p[0], p[1]])),
                Cmd::Move(p) => e.move_to(x.apply2([p[0], p[1]])),
                Cmd::Curve4 { end, c1, c2 } => {
                    e.curve_to(x.apply2([c1[0], c1[1]]), x.apply2([c2[0], c2[1]]), x.apply2([end[0], end[1]]))
                }
                Cmd::Curve3 { end, ctrl } => e.quad_to(x.apply2([ctrl[0], ctrl[1]]), x.apply2([end[0], end[1]])),
            }
        }
        e
    }

    /// Polyline vertices of a path with curves flattened (xy only, WCS).
    pub fn flatten(&self, distance: f64, segments: usize) -> Vec<V3> {
        let mut out = vec![self.start];
        let mut cur = self.start;
        for c in &self.cmds {
            match *c {
                Cmd::Line(p) | Cmd::Move(p) => {
                    out.push(p);
                    cur = p;
                }
                Cmd::Curve4 { end, c1, c2 } => {
                    let mut pts = Vec::new();
                    flatten_cubic(&[[cur[0], cur[1]], [c1[0], c1[1]], [c2[0], c2[1]], [end[0], end[1]]], distance, segments, &mut pts);
                    let n = pts.len().max(1);
                    for (i, p) in pts.iter().enumerate() {
                        let t = (i + 1) as f64 / n as f64;
                        out.push([p[0], p[1], cur[2] + (end[2] - cur[2]) * t]);
                    }
                    cur = end;
                }
                Cmd::Curve3 { end, ctrl } => {
                    let c1 = add(cur, mul(sub(ctrl, cur), 2.0 / 3.0));
                    let c2 = add(end, mul(sub(ctrl, end), 2.0 / 3.0));
                    let mut pts = Vec::new();
                    flatten_cubic(&[[cur[0], cur[1]], [c1[0], c1[1]], [c2[0], c2[1]], [end[0], end[1]]], distance, segments, &mut pts);
                    let n = pts.len().max(1);
                    for (i, p) in pts.iter().enumerate() {
                        let t = (i + 1) as f64 / n as f64;
                        out.push([p[0], p[1], cur[2] + (end[2] - cur[2]) * t]);
                    }
                    cur = end;
                }
            }
        }
        out
    }
}

fn reverse_simple(p: &Path) -> Path {
    // points of the command chain from the end to the start
    let mut pts = vec![p.start];
    for c in &p.cmds {
        pts.push(c.end());
    }
    let mut out = Path::new(*pts.last().unwrap_or(&p.start));
    for (i, c) in p.cmds.iter().enumerate().rev() {
        let target = pts[i];
        match *c {
            Cmd::Line(_) | Cmd::Move(_) => out.line_to(target),
            Cmd::Curve3 { ctrl, .. } => out.curve3_to(target, ctrl),
            Cmd::Curve4 { c1, c2, .. } => out.curve4_to(target, c2, c1),
        }
    }
    out
}

/// Port of `ezdxf.path.tools.add_bezier4p`: connects the curves to the end of `path`, reversing them when that fits better.
pub fn add_bezier4p(path: &mut Path, mut curves: Vec<[V3; 4]>) {
    if curves.is_empty() {
        return;
    }
    let end = curves[curves.len() - 1][3];
    if v3_close(path.end(), end) {
        curves.reverse();
        for c in curves.iter_mut() {
            c.reverse();
        }
    }
    for c in curves {
        let [start, c1, c2, end] = c;
        if !v3_close(start, path.end()) {
            path.line_to(start);
        }
        if v3_close_tol(start, c1, 1e-15, 0.0) && v3_close_tol(end, c2, 1e-15, 0.0) {
            path.line_to(end);
        } else {
            path.curve4_to(end, c1, c2);
        }
    }
}

/// Port of `ezdxf.path.tools.add_ellipse`.
pub fn add_ellipse(path: &mut Path, center: V3, major: V3, minor: V3, start_param: f64, end_param: f64, segments: usize, reset: bool) {
    let span = arc_span_rad(start_param, end_param);
    if span.abs() < 1e-9 {
        return;
    }
    if path.is_empty() && reset {
        path.start = add(center, add(mul(major, start_param.cos()), mul(minor, start_param.sin())));
    }
    add_bezier4p(path, ellipse_beziers(center, major, minor, start_param, end_param, segments));
}

/// Path of an arc / circle given in OCS (ezdxf `ConstructionEllipse.from_arc` + `add_ellipse`).
pub fn arc_path(center: V3, radius: f64, extrusion: V3, start_deg: f64, end_deg: f64) -> Path {
    let mut path = Path::default();
    let radius = radius.abs();
    if radius <= 1e-12 {
        return path;
    }
    let ocs = Ocs::new(extrusion);
    let c = ocs.to_wcs(center);
    let major = ocs.to_wcs([radius, 0.0, 0.0]);
    let minor = minor_axis(major, normalize_or_z(extrusion), 1.0);
    add_ellipse(&mut path, c, major, minor, start_deg.to_radians(), end_deg.to_radians(), 1, true);
    path
}

pub fn normalize_or_z(v: V3) -> V3 {
    if is_null(v) {
        Z_AXIS
    } else {
        normalize(v)
    }
}

/// ezdxf `tools.add_2d_polyline`: vertices (x, y, bulge) in OCS to an (empty) path in WCS.
pub fn polyline_path(points: &[(f64, f64, f64)], close: bool, ocs: &Ocs, elevation: f64) -> Path {
    let mut path = Path::default();
    let mut prev: Option<V3> = None;
    let mut prev_bulge = 0.0;
    let bulge_to = |path: &mut Path, p1: V3, p2: V3, bulge: f64| {
        if v3_close_tol(p1, p2, 1e-9, 0.0) {
            return;
        }
        let (c, sa, ea, r) = bulge_to_arc([p1[0], p1[1]], [p2[0], p2[1]], bulge);
        let sa = fmod(sa, TAU);
        let mut ea = fmod(ea, TAU);
        if sa > ea {
            ea += TAU;
        }
        // one cubic Bezier per started quarter (num_bez = ceil(segments / 3) = 1 piece, split by the arc rule)
        let major = [r, 0.0, 0.0];
        let minor = [0.0, r, 0.0];
        let center = [c[0], c[1], 0.0];
        let mut curves = ellipse_beziers(center, major, minor, sa, ea, 1);
        if curves.is_empty() {
            return;
        }
        if v3_close_tol(curves[0][0], p2, 1e-9, 0.0) {
            curves.reverse();
            for c in curves.iter_mut() {
                c.reverse();
            }
        }
        add_bezier4p(path, curves);
    };
    for &(x, y, bulge) in points {
        let b = if bulge.abs() < 1e-6 { 0.0 } else { bulge };
        let p = [x, y, 0.0];
        match prev {
            None => {
                path.start = p;
            }
            Some(pp) => {
                if prev_bulge != 0.0 {
                    bulge_to(&mut path, pp, p, prev_bulge);
                } else {
                    path.line_to(p);
                }
            }
        }
        prev = Some(p);
        prev_bulge = b;
    }
    if close && prev.is_some() && !v3_close_tol(path.start, path.end(), 1e-9, 0.0) {
        let e = path.end();
        let s0 = path.start;
        if prev_bulge != 0.0 {
            bulge_to(&mut path, e, s0, prev_bulge);
        } else {
            path.line_to(path.start);
        }
    }
    if ocs.transform || elevation != 0.0 {
        let x = Xform::translate([0.0, 0.0, elevation]).then(&ocs.xform());
        path = path.transformed(&x);
    }
    path
}

/// `from_vertices`: a path through the points, repeated points skipped.
pub fn path_from_vertices(vertices: &[V3], close: bool) -> Path {
    if vertices.len() < 2 {
        return Path::default();
    }
    let mut p = Path::new(vertices[0]);
    for &v in &vertices[1..] {
        if !v3_close(p.end(), v) {
            p.line_to(v);
        }
    }
    if close {
        p.close();
    }
    p
}

// ------------------------------------------------------------------------------------------------ 2D polygon helpers
/// Even-odd point in polygon test (ring need not be closed).
pub fn point_in_ring(p: V2, ring: &[V2]) -> bool {
    let mut inside = false;
    let n = ring.len();
    if n < 3 {
        return false;
    }
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (ring[i], ring[j]);
        if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0] {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Signed area (positive for counter-clockwise rings).
pub fn ring_area(ring: &[V2]) -> f64 {
    let n = ring.len();
    let mut a = 0.0;
    for i in 0..n {
        let (p, q) = (ring[i], ring[(i + 1) % n]);
        a += p[0] * q[1] - q[0] * p[1];
    }
    a * 0.5
}

/// Squared distance of point `p` to segment `a`-`b` and the closest point.
pub fn seg_dist2(p: V2, a: V2, b: V2) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l2 = dx * dx + dy * dy;
    let t = if l2 > 0.0 { (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
    let (cx, cy) = (a[0] + dx * t, a[1] + dy * t);
    (p[0] - cx).powi(2) + (p[1] - cy).powi(2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ocs_arbitrary_axis() {
        let o = Ocs::new([0.0, 0.0, -1.0]);
        let w = o.to_wcs([1.0, 2.0, 0.0]);
        assert!((w[0] + 1.0).abs() < 1e-12 && (w[1] - 2.0).abs() < 1e-12 && w[2].abs() < 1e-12);
        let o = Ocs::new([0.0, 0.0, 1.0]);
        assert!(!o.transform);
        let o = Ocs::new([1.0, 0.0, 0.0]);
        let w = o.to_wcs([0.0, 0.0, 1.0]);
        assert!((w[0] - 1.0).abs() < 1e-12);
        let back = o.from_wcs(o.to_wcs([3.0, 4.0, 5.0]));
        assert!((back[0] - 3.0).abs() < 1e-12 && (back[1] - 4.0).abs() < 1e-12 && (back[2] - 5.0).abs() < 1e-12);
    }

    #[test]
    fn arc_spans() {
        assert!((arc_span_deg(0.0, 360.0) - 360.0).abs() < 1e-12);
        assert!((arc_span_deg(350.0, 10.0) - 20.0).abs() < 1e-9);
        assert_eq!(arc_span_deg(10.0, 10.0), 0.0);
        assert!((arc_span_rad(PI, -PI) - TAU).abs() < 1e-12);
    }

    #[test]
    fn bulge_semicircle() {
        let (c, a0, a1, r) = bulge_to_arc([0.0, 0.0], [2.0, 0.0], 1.0);
        assert!((c[0] - 1.0).abs() < 1e-12 && c[1].abs() < 1e-12 && (r - 1.0).abs() < 1e-12);
        assert!((arc_span_rad(a0, a1) - PI).abs() < 1e-9);
        // negative bulge swaps the angles
        let (c, _, _, _) = bulge_to_arc([0.0, 0.0], [2.0, 0.0], -1.0);
        assert!((c[0] - 1.0).abs() < 1e-12);
    }

    #[test]
    fn circle_path_is_four_curves_and_reversed() {
        let p = arc_path([0.0, 0.0, 0.0], 5.0, Z_AXIS, 0.0, 360.0);
        assert_eq!(p.len(), 4);
        // a full circle starts and ends at (r, 0)
        assert!(v3_close_tol(p.start, [5.0, 0.0, 0.0], 1e-9, 1e-9));
        assert!(v3_close_tol(p.end(), [5.0, 0.0, 0.0], 1e-9, 1e-9));
        // reversed: the first curve goes clockwise (to (0, -r))
        if let Cmd::Curve4 { end, .. } = p.cmds[0] {
            assert!((end[0]).abs() < 1e-9 && (end[1] + 5.0).abs() < 1e-9, "{:?}", end);
        } else {
            panic!("curve expected");
        }
    }

    #[test]
    fn xform_composition_order() {
        let a = Xform::scale([2.0, 2.0, 1.0]);
        let b = Xform::translate([1.0, 0.0, 0.0]);
        let p = a.then(&b).apply([1.0, 1.0, 0.0]);
        assert_eq!(p, [3.0, 2.0, 0.0]);
        assert!((a.then(&b).uniform_scale_2d().unwrap() - 2.0).abs() < 1e-12);
    }

    #[test]
    fn flatten_semicircle_within_tolerance() {
        let p = arc_path([0.0, 0.0, 0.0], 10.0, Z_AXIS, 0.0, 180.0);
        let pts = p.flatten(0.01, 4);
        for q in &pts {
            let r = (q[0] * q[0] + q[1] * q[1]).sqrt();
            assert!((r - 10.0).abs() < 0.02, "r={}", r);
        }
        assert!(pts.len() > 8);
    }

    #[test]
    fn polyline_with_bulge_makes_arc() {
        let p = polyline_path(&[(0.0, 0.0, 1.0), (2.0, 0.0, 0.0)], false, &Ocs::new(Z_AXIS), 0.0);
        assert_eq!(p.len(), 2); // a semicircle is two quarter curves
        assert!(v3_close_tol(p.end(), [2.0, 0.0, 0.0], 1e-9, 1e-9));
    }
}
