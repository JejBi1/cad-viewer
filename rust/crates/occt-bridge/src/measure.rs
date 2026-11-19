//! Picked-entity measurement: a port of `stepview/analysis.py` (Entity, relation, summary) and of the way
//! `main.py: on_entities` assembles the rows. The OCCT facts (adaptor data, BRepExtrema distance) come from the
//! C++ side; everything else, including all Czech texts and tolerances, lives here and is unit tested.

use cad_types::{EntityKind, EntityRef, P3};

use crate::ffi::ffi as raw;

// ------------------------------------------------------------------ number formatting (analysis.py: _p, _pt, _dir)

/// `f"{v:,.{d}f}".replace(",", " ").replace(".", ",")`
pub fn fmt_p(v: f64, d: usize) -> String {
    if v.is_nan() {
        return "nan".to_string();
    }
    if v.is_infinite() {
        return if v < 0.0 {
            "-inf".to_string()
        } else {
            "inf".to_string()
        };
    }
    let s = format!("{v:.d$}");
    let (neg, rest) = match s.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, s.as_str()),
    };
    let (int, frac) = match rest.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (rest, None),
    };
    let mut out = String::with_capacity(s.len() + 4);
    if neg {
        out.push('-');
    }
    let n = int.len();
    for (i, ch) in int.chars().enumerate() {
        if i > 0 && (n - i) % 3 == 0 {
            out.push(' ');
        }
        out.push(ch);
    }
    if let Some(f) = frac {
        out.push(',');
        out.push_str(f);
    }
    out
}

/// main.py `fmt_len`: two decimals.
pub fn fmt_len(v: f64) -> String {
    fmt_p(v, 2)
}

/// `f"{n:,}".replace(",", " ")` for integers.
pub fn fmt_thousands(n: u64) -> String {
    fmt_p(n as f64, 0)
}

fn fmt_pt(p: &P3) -> String {
    format!("{}; {}; {}", fmt_p(p[0], 3), fmt_p(p[1], 3), fmt_p(p[2], 3))
}

/// `f"({x:.4f}; {y:.4f}; {z:.4f})".replace(".", ",")`
fn fmt_dir(d: &P3) -> String {
    format!("({:.4}; {:.4}; {:.4})", d[0], d[1], d[2]).replace('.', ",")
}

// ------------------------------------------------------------------ vector helpers (gp_Vec / gp_Dir semantics)

fn sub(a: &P3, b: &P3) -> P3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: &P3, b: &P3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: &P3, b: &P3) -> P3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn modulus(a: &P3) -> f64 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}

fn distance(a: &P3, b: &P3) -> f64 {
    let d = sub(a, b);
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

/// `gp_Dir(v)`: normalised, `None` for a null vector (OCCT raises there).
pub(crate) fn normalized(v: &P3) -> Option<P3> {
    let m = modulus(v);
    if m <= 1e-12 {
        None
    } else {
        Some([v[0] / m, v[1] / m, v[2] / m])
    }
}

/// `gp_Dir::Angle`: accurate near 0 and near pi (acos in the middle, asin at the ends). Radians, 0..pi.
/// The switch value is OCCT's own truncated constant, not `FRAC_1_SQRT_2`.
#[allow(clippy::approx_constant)]
fn dir_angle(a: &P3, b: &P3) -> f64 {
    let cosinus = dot(a, b);
    if cosinus > -0.70710678118655 && cosinus < 0.70710678118655 {
        cosinus.acos()
    } else {
        let sinus = modulus(&cross(a, b));
        if cosinus < 0.0 {
            std::f64::consts::PI - sinus.asin()
        } else {
            sinus.asin()
        }
    }
}

/// `math.degrees(x)`.
fn degrees(x: f64) -> f64 {
    x * (180.0 / std::f64::consts::PI)
}

// ------------------------------------------------------------------ rows

/// One line of the properties panel. `bold` replaces the `<b>..</b>` markup of the Python rows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MeasureRow {
    pub label: String,
    pub value: String,
    pub bold: bool,
}

impl MeasureRow {
    pub fn new(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            bold: false,
        }
    }
    pub fn bold(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            bold: true,
        }
    }
}

// ------------------------------------------------------------------ entity

/// `Entity.kind` of analysis.py.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeasureKind {
    Vertex,
    Edge,
    Face,
    /// A picked point (exact B-rep point or a mesh point).
    Point,
}

impl MeasureKind {
    /// `kind_cz()`
    pub fn cz(self) -> &'static str {
        match self {
            MeasureKind::Vertex => "vrchol",
            MeasureKind::Edge => "hrana",
            MeasureKind::Face => "plocha",
            MeasureKind::Point => "bod",
        }
    }
}

/// A flat patch of an STL mesh under a picked point. Patches with the same `id` are the same patch (the Python
/// code compares the identity of the facet list when it sums areas).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeshPatch {
    pub id: u64,
    pub facets: usize,
    pub area: f64,
    pub centroid: P3,
}

/// Fitted circle of a mesh outline (`meshmeasure.Loop.circle`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LoopCircle {
    pub center: P3,
    pub normal: P3,
    pub radius: f64,
    pub rms: f64,
}

/// A sharp-edge outline of an STL mesh (`meshmeasure.Loop`).
#[derive(Clone, Debug, PartialEq)]
pub struct MeshLoop {
    /// Polyline points, at least two.
    pub points: Vec<P3>,
    pub closed: bool,
    pub length: f64,
    /// `Loop.kind()`: "obrys", "kružnice" or "oblouk".
    pub kind: String,
    pub circle: Option<LoopCircle>,
    /// Angle covered by the fitted circle, degrees.
    pub coverage: f64,
}

/// What the measure tool can pick.
#[derive(Clone, Debug, PartialEq)]
pub enum MeasureItem {
    /// A vertex, edge or face of a placed instance.
    Entity(EntityRef),
    /// A point on a face (exact, with its normal) or on a mesh.
    Point {
        point: P3,
        normal: Option<P3>,
        mesh: bool,
        snapped: bool,
        patch: Option<MeshPatch>,
    },
    /// A sharp-edge outline of a mesh.
    Loop(MeshLoop),
}

/// A picked sub-shape in world coordinates plus what is known about it (analysis.py: `Entity`).
#[derive(Clone, Debug, PartialEq)]
pub struct Entity {
    pub kind: MeasureKind,
    /// "kružnice", "rovina", ...
    pub type_name: String,
    /// Vertex / picked point.
    pub point: Option<P3>,
    /// Circle / sphere / arc centre, cylinder axis point, plane point.
    pub center: Option<P3>,
    /// Line direction, circle / cylinder axis or plane normal (unit vector).
    pub axis: Option<P3>,
    pub radius: Option<f64>,
    pub length: Option<f64>,
    pub area: Option<f64>,
    /// `axis` is a line direction (edge line, cylinder axis), not a normal.
    pub is_line: bool,
    /// Arc angle in degrees (circles and mesh outlines).
    pub arc: Option<f64>,
    pub axes: Option<(f64, f64)>,
    pub half_angle: Option<f64>,
    pub radii: Option<(f64, f64)>,
    pub centroid: Option<P3>,
    pub plane_point: Option<P3>,
    pub patch: Option<MeshPatch>,
    pub loop_closed: Option<bool>,
    pub fit_rms: Option<f64>,
    pub p0: Option<P3>,
    pub p1: Option<P3>,
}

impl Entity {
    fn empty(kind: MeasureKind, type_name: &str) -> Self {
        Entity {
            kind,
            type_name: type_name.to_string(),
            point: None,
            center: None,
            axis: None,
            radius: None,
            length: None,
            area: None,
            is_line: false,
            arc: None,
            axes: None,
            half_angle: None,
            radii: None,
            centroid: None,
            plane_point: None,
            patch: None,
            loop_closed: None,
            fit_rms: None,
            p0: None,
            p1: None,
        }
    }

    /// `Entity.point`: a picked point, optionally with a surface normal (then it acts as a plane for thickness).
    pub fn picked_point(
        point: P3,
        normal: Option<P3>,
        mesh: bool,
        snapped: bool,
        patch: Option<MeshPatch>,
    ) -> Result<Self, String> {
        let type_name = if mesh {
            if snapped {
                "vrchol sítě"
            } else {
                "bod na síti"
            }
        } else {
            "bod na ploše"
        };
        let mut e = Entity::empty(MeasureKind::Point, type_name);
        e.point = Some(point);
        if let Some(n) = normal {
            e.axis = Some(normalized(&n).ok_or_else(|| "nulová normála bodu".to_string())?);
            e.plane_point = Some(point);
        }
        if let Some(p) = patch {
            e.patch = Some(p);
            e.type_name.push_str(", rovina");
        }
        Ok(e)
    }

    /// `Entity.loop`: a sharp-edge outline of a mesh; holes and pins come with a fitted circle.
    pub fn mesh_loop(lp: &MeshLoop) -> Result<Self, String> {
        let first = lp
            .points
            .first()
            .ok_or_else(|| "prázdný obrys sítě".to_string())?;
        let last = lp.points.last().unwrap_or(first);
        let mut e = Entity::empty(MeasureKind::Edge, &format!("{} (síť)", lp.kind));
        e.length = Some(lp.length);
        e.p0 = Some(*first);
        e.p1 = Some(*last);
        e.loop_closed = Some(lp.closed);
        if let Some(c) = &lp.circle {
            e.center = Some(c.center);
            e.radius = Some(c.radius);
            e.fit_rms = Some(c.rms);
            e.axis =
                Some(normalized(&c.normal).ok_or_else(|| "nulová normála kružnice".to_string())?);
            e.arc = Some(lp.coverage);
        }
        Ok(e)
    }

    /// Entity of the B-rep from the raw data delivered by the C++ side.
    pub(crate) fn from_rec(r: &raw::EntityRec) -> Self {
        match r.kind {
            0 => {
                let mut e = Entity::empty(MeasureKind::Vertex, "bod");
                e.point = Some(r.point);
                e
            }
            1 => {
                let name = match r.geom {
                    1 => "úsečka",
                    2 => "kružnice",
                    3 => "elipsa",
                    4 => "spline",
                    _ => "křivka",
                };
                let mut e = Entity::empty(MeasureKind::Edge, name);
                if r.has_length {
                    e.length = Some(r.length);
                }
                if r.has_ends {
                    e.p0 = Some(r.p0);
                    e.p1 = Some(r.p1);
                }
                if r.has_axis {
                    e.axis = Some(r.axis);
                }
                if r.has_center {
                    e.center = Some(r.center);
                }
                if r.has_radius {
                    e.radius = Some(r.radius);
                }
                e.is_line = r.is_line;
                if r.has_arc {
                    e.arc = Some(r.arc_deg);
                    // arc vs circle: analysis.py `if self.arc < 359.9`
                    if r.arc_deg < 359.9 {
                        e.type_name = "oblouk".to_string();
                    }
                }
                if r.has_axes {
                    e.axes = Some((r.axes[0], r.axes[1]));
                }
                e
            }
            _ => {
                let name = match r.geom {
                    10 => "rovina",
                    11 => "válec",
                    12 => "kužel",
                    13 => "koule",
                    14 => "anuloid",
                    15 => "spline plocha",
                    _ => "plocha",
                };
                let mut e = Entity::empty(MeasureKind::Face, name);
                if r.has_area {
                    e.area = Some(r.area);
                    e.centroid = Some(r.centroid);
                }
                if r.has_axis {
                    e.axis = Some(r.axis);
                }
                if r.has_center {
                    e.center = Some(r.center);
                }
                if r.has_radius {
                    e.radius = Some(r.radius);
                }
                e.is_line = r.is_line;
                if r.has_half_angle {
                    e.half_angle = Some(r.half_angle);
                }
                if r.has_radii {
                    e.radii = Some((r.radii[0], r.radii[1]));
                }
                if r.has_plane_point {
                    e.plane_point = Some(r.plane_point);
                }
                e
            }
        }
    }

    pub fn kind_cz(&self) -> &'static str {
        self.kind.cz()
    }

    /// `patch_area`: area of the flat mesh patch under a picked point.
    pub fn patch_area(&self) -> Option<f64> {
        self.patch.map(|p| p.area)
    }

    /// One line for the hover preview (`short()`).
    pub fn short(&self) -> String {
        let mut t = format!("{}: {}", self.kind_cz(), self.type_name);
        if let Some(r) = self.radius {
            t.push_str(&format!("  Ø {}  R {}", fmt_p(2.0 * r, 3), fmt_p(r, 3)));
        }
        if self.kind == MeasureKind::Edge
            && let Some(l) = self.length
        {
            t.push_str(&format!("  L {}", fmt_p(l, 3)));
        }
        if self.kind == MeasureKind::Face
            && let Some(a) = self.area
        {
            t.push_str(&format!("  A {} mm²", fmt_p(a, 2)));
        }
        if matches!(self.kind, MeasureKind::Vertex | MeasureKind::Point)
            && let Some(p) = &self.point
        {
            t.push_str(&format!("  ({})", fmt_pt(p)));
        }
        if let Some(a) = self.patch_area() {
            t.push_str(&format!("  A {} mm²", fmt_p(a, 2)));
        }
        t
    }

    /// Where a 3D label for this entity goes, and its text (`label_pos()`).
    pub fn label_pos(&self) -> Option<(P3, String)> {
        match self.kind {
            MeasureKind::Edge => {
                let (p0, p1) = (self.p0?, self.p1?);
                let p = if self.radius.is_some() {
                    p0
                } else {
                    [
                        (p0[0] + p1[0]) / 2.0,
                        (p0[1] + p1[1]) / 2.0,
                        (p0[2] + p1[2]) / 2.0,
                    ]
                };
                let txt = match self.radius {
                    Some(r) => format!("Ø {}", fmt_p(2.0 * r, 2)),
                    None => fmt_p(self.length?, 2),
                };
                Some((p, txt))
            }
            MeasureKind::Face => {
                let txt = match self.radius {
                    Some(r) => format!("Ø {}", fmt_p(2.0 * r, 2)),
                    None => format!("{} mm²", fmt_p(self.area?, 1)),
                };
                Some((self.centroid?, txt))
            }
            _ => {
                let p = self.patch?;
                Some((p.centroid, format!("{} mm²", fmt_p(p.area, 1))))
            }
        }
    }

    /// Rows of the properties panel for a single entity (`rows()`).
    pub fn rows(&self) -> Vec<MeasureRow> {
        let mut r = vec![MeasureRow::new(
            "Entita",
            format!("{} – {}", self.kind_cz(), self.type_name),
        )];
        if matches!(self.kind, MeasureKind::Vertex | MeasureKind::Point) {
            if let Some(p) = &self.point {
                r.push(MeasureRow::new("X; Y; Z", fmt_pt(p)));
            }
            if self.kind == MeasureKind::Point
                && let Some(ax) = &self.axis
            {
                r.push(MeasureRow::new("Normála", fmt_dir(ax)));
            }
            if let Some(patch) = &self.patch {
                let a = patch.area;
                r.push(MeasureRow::new(
                    "Rovinná plocha (síť)",
                    format!(
                        "{} mm²  ({} cm², {} trojúh.)",
                        fmt_p(a, 2),
                        fmt_p(a / 100.0, 3),
                        patch.facets
                    ),
                ));
            }
        }
        if self.kind == MeasureKind::Edge {
            if let Some(l) = self.length {
                r.push(MeasureRow::new("Délka", format!("{} mm", fmt_p(l, 3))));
            }
            if let Some(rad) = self.radius {
                r.push(MeasureRow::new(
                    "Poloměr R",
                    format!("{} mm", fmt_p(rad, 3)),
                ));
                r.push(MeasureRow::new(
                    "Průměr Ø",
                    format!("{} mm", fmt_p(2.0 * rad, 3)),
                ));
                if let Some(c) = &self.center {
                    r.push(MeasureRow::new("Střed", fmt_pt(c)));
                }
                if let Some(ax) = &self.axis {
                    r.push(MeasureRow::new("Osa", fmt_dir(ax)));
                }
                if self.type_name.starts_with("oblouk")
                    && let Some(arc) = self.arc
                {
                    r.push(MeasureRow::new(
                        "Úhel oblouku",
                        format!("{}°", fmt_p(arc, 2)),
                    ));
                }
                if let Some(rms) = self.fit_rms {
                    r.push(MeasureRow::new(
                        "Proložení kružnice",
                        format!("± {} mm (ze sítě)", fmt_p(rms, 3)),
                    ));
                }
            } else if self.is_line
                && let (Some(ax), Some(p0), Some(p1)) = (&self.axis, &self.p0, &self.p1)
            {
                let d = sub(p1, p0);
                r.push(MeasureRow::new("Směr", fmt_dir(ax)));
                r.push(MeasureRow::new(
                    "ΔX; ΔY; ΔZ",
                    format!("{}; {}; {}", fmt_p(d[0], 3), fmt_p(d[1], 3), fmt_p(d[2], 3)),
                ));
            }
            if let Some((a, b)) = self.axes {
                r.push(MeasureRow::new(
                    "Poloosy",
                    format!("{} / {} mm", fmt_p(a, 3), fmt_p(b, 3)),
                ));
            }
            if let Some(p0) = &self.p0 {
                r.push(MeasureRow::new("Začátek", fmt_pt(p0)));
            }
            if let Some(p1) = &self.p1 {
                r.push(MeasureRow::new("Konec", fmt_pt(p1)));
            }
        }
        if self.kind == MeasureKind::Face {
            if let Some(a) = self.area {
                r.push(MeasureRow::new(
                    "Plocha",
                    format!("{} mm²  ({} cm²)", fmt_p(a, 2), fmt_p(a / 100.0, 3)),
                ));
            }
            if let Some(rad) = self.radius {
                r.push(MeasureRow::new(
                    "Poloměr R",
                    format!("{} mm", fmt_p(rad, 3)),
                ));
                r.push(MeasureRow::new(
                    "Průměr Ø",
                    format!("{} mm", fmt_p(2.0 * rad, 3)),
                ));
            }
            if self.type_name == "rovina" {
                if let Some(ax) = &self.axis {
                    r.push(MeasureRow::new("Normála", fmt_dir(ax)));
                }
            } else if let Some(ax) = &self.axis {
                r.push(MeasureRow::new("Osa", fmt_dir(ax)));
            }
            if self.type_name == "kužel"
                && let Some(h) = self.half_angle
            {
                r.push(MeasureRow::new(
                    "Poloviční úhel",
                    format!("{}°", fmt_p(h, 2)),
                ));
            }
            if let Some((a, b)) = self.radii {
                r.push(MeasureRow::new(
                    "R / r",
                    format!("{} / {} mm", fmt_p(a, 3), fmt_p(b, 3)),
                ));
            }
            if let Some(c) = &self.centroid {
                r.push(MeasureRow::new("Těžiště plochy", fmt_pt(c)));
            }
        }
        r
    }

    /// Characteristic point for centre-to-centre distances (`ref_point()`).
    pub fn ref_point(&self) -> Option<P3> {
        if matches!(self.kind, MeasureKind::Vertex | MeasureKind::Point) {
            return self.point;
        }
        if self.radius.is_some() && self.center.is_some() && self.kind == MeasureKind::Edge {
            return self.center;
        }
        if self.type_name == "koule" {
            return self.center;
        }
        None
    }
}

// ------------------------------------------------------------------ two entities

/// Result of `BRepExtrema_DistShapeShape` (first solution), when it found one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Distance {
    pub value: f64,
    pub p1: P3,
    pub p2: P3,
}

/// Drawing data of `relation()`: the closest points and the points the centre distance runs between.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Draw {
    pub min: Option<(P3, P3)>,
    pub center: Option<(P3, P3)>,
}

/// Distance of two (infinite) lines; for parallel lines the perpendicular distance. Returns `(distance, parallel)`.
pub fn line_line_distance(p1: &P3, d1: &P3, p2: &P3, d2: &P3) -> (f64, bool) {
    let w = sub(p2, p1);
    let n = cross(d1, d2);
    if modulus(&n) < 1e-9 {
        (modulus(&cross(&w, d1)) / modulus(d1), true)
    } else {
        (dot(&w, &n).abs() / modulus(&n), false)
    }
}

/// Rows plus drawing data for two entities: minimum distance (+ global ΔX/ΔY/ΔZ), centre distance, angle,
/// plane spacing / thickness, axis distance (`relation(a, b)`). `dist` is the BRepExtrema result.
pub fn relation(a: &Entity, b: &Entity, dist: Option<Distance>) -> (Vec<MeasureRow>, Draw) {
    let mut rows = Vec::new();
    let mut draw = Draw::default();
    if let Some(d) = dist {
        let v = sub(&d.p2, &d.p1);
        rows.push(MeasureRow::bold(
            "Min. vzdálenost",
            format!("{} mm", fmt_p(d.value, 3)),
        ));
        rows.push(MeasureRow::new(
            "ΔX / ΔY / ΔZ",
            format!(
                "{} / {} / {} mm",
                fmt_p(v[0], 3),
                fmt_p(v[1], 3),
                fmt_p(v[2], 3)
            ),
        ));
        rows.push(MeasureRow::new("Bod 1", fmt_pt(&d.p1)));
        rows.push(MeasureRow::new("Bod 2", fmt_pt(&d.p2)));
        draw.min = Some((d.p1, d.p2));
    }
    if let (Some(ra), Some(rb)) = (a.ref_point(), b.ref_point()) {
        let d = sub(&rb, &ra);
        rows.push(MeasureRow::new(
            "Vzdálenost středů",
            format!("{} mm", fmt_p(distance(&ra, &rb), 3)),
        ));
        rows.push(MeasureRow::new(
            "Středy ΔX / ΔY / ΔZ",
            format!(
                "{} / {} / {} mm",
                fmt_p(d[0], 3),
                fmt_p(d[1], 3),
                fmt_p(d[2], 3)
            ),
        ));
        draw.center = Some((ra, rb));
    }
    if let (Some(aa), Some(ba)) = (&a.axis, &b.axis) {
        let mut ang = degrees(dir_angle(aa, ba));
        if a.is_line == b.is_line {
            // line-line or normal-normal
            if a.is_line {
                ang = ang.min(180.0 - ang);
            }
            rows.push(MeasureRow::new("Úhel", format!("{}°", fmt_p(ang, 3))));
        } else {
            // line vs plane normal -> line to plane angle
            rows.push(MeasureRow::new(
                "Úhel (přímka – rovina)",
                format!("{}°", fmt_p((90.0 - ang.min(180.0 - ang)).abs(), 3)),
            ));
        }
        // Parallel planes (B-rep planes or picked points with their surface normal) -> perpendicular distance =
        // wall thickness / plane spacing. Mesh normals are noisy, so 2 degrees of tolerance there.
        let both_planes = a.type_name == "rovina" && b.type_name == "rovina";
        let tol = if both_planes { 1e-4 } else { 2.0 };
        if let (Some(pa), Some(pb)) = (&a.plane_point, &b.plane_point)
            && ang.min(180.0 - ang) < tol
        {
            let dist = dot(&sub(pb, pa), aa).abs();
            let label = if both_planes {
                "Vzdálenost rovin"
            } else {
                "Kolmá vzdálenost (tloušťka)"
            };
            rows.push(MeasureRow::bold(label, format!("{} mm", fmt_p(dist, 3))));
        }
        if a.is_line
            && b.is_line
            && let (Some(ca), Some(cb)) = (&a.center, &b.center)
        {
            let (dist, par) = line_line_distance(ca, aa, cb, ba);
            let name = if par {
                "Vzdálenost os (rovnoběžné)"
            } else {
                "Vzdálenost os (mimoběžné)"
            };
            rows.push(MeasureRow::new(name, format!("{} mm", fmt_p(dist, 3))));
        }
    }
    (rows, draw)
}

/// Three or more entities: totals (`summary(ents)`).
pub fn summary(ents: &[Entity]) -> Vec<MeasureRow> {
    let mut rows = vec![MeasureRow::new("Vybráno", format!("{} entit", ents.len()))];
    let edges: Vec<&Entity> = ents
        .iter()
        .filter(|e| e.kind == MeasureKind::Edge)
        .collect();
    let faces: Vec<&Entity> = ents
        .iter()
        .filter(|e| e.kind == MeasureKind::Face)
        .collect();
    if !edges.is_empty() {
        let total = edges.iter().fold(0.0, |s, e| s + e.length.unwrap_or(0.0));
        rows.push(MeasureRow::new(
            "Součet délek hran",
            format!("{} mm ({} hran)", fmt_p(total, 3), edges.len()),
        ));
    }
    if !faces.is_empty() {
        let a = faces.iter().fold(0.0, |s, e| s + e.area.unwrap_or(0.0));
        rows.push(MeasureRow::new(
            "Součet ploch",
            format!(
                "{} mm²  ({} cm², {} ploch)",
                fmt_p(a, 2),
                fmt_p(a / 100.0, 3),
                faces.len()
            ),
        ));
    }
    // one entry per distinct patch (dict keyed by the identity of the facet list: the last area wins)
    let mut patches: Vec<(u64, f64)> = Vec::new();
    for e in ents {
        if let Some(p) = &e.patch {
            match patches.iter_mut().find(|(id, _)| *id == p.id) {
                Some(slot) => slot.1 = p.area,
                None => patches.push((p.id, p.area)),
            }
        }
    }
    if !patches.is_empty() {
        let a = patches.iter().fold(0.0, |s, (_, v)| s + v);
        rows.push(MeasureRow::new(
            "Součet rovinných ploch (síť)",
            format!(
                "{} mm²  ({} cm², {} ploch)",
                fmt_p(a, 2),
                fmt_p(a / 100.0, 3),
                patches.len()
            ),
        ));
    }
    rows
}

// ------------------------------------------------------------------ the whole measurement (main.py: on_entities)

/// Everything the measure tool shows for the current selection.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Measurement {
    /// One entry per picked item.
    pub entities: Vec<Entity>,
    /// Rows of the "Měření" panel exactly as `main.on_entities` assembles them (without the title row and without
    /// the usage hints, which are UI text).
    pub rows: Vec<MeasureRow>,
    /// One entity: position and text of the 3D label (`label_pos`).
    pub label: Option<(P3, String)>,
    /// Status-bar text (`st_model`) for one or two entities.
    pub status: Option<String>,
    /// Two entities: the closest points (`draw["min"]`) and the points of the centre distance.
    pub min_points: Option<(P3, P3)>,
    pub center_points: Option<(P3, P3)>,
    /// Two entities with a minimum distance: text of the dimension line in the 3D view (`"{d:.3f} mm"`, comma).
    pub distance_label: Option<String>,
    /// Set when the selection could not be evaluated; `rows` then holds the "Měření / nelze vyhodnotit" row.
    pub error: Option<String>,
}

impl Measurement {
    pub(crate) fn failed(message: &str) -> Self {
        Measurement {
            rows: vec![MeasureRow::new(
                "Měření",
                format!("nelze vyhodnotit: {message}"),
            )],
            error: Some(message.to_string()),
            ..Measurement::default()
        }
    }
}

/// Assemble the panel for already evaluated entities. `dist` is only used for two entities.
pub fn assemble(ents: Vec<Entity>, dist: Option<Distance>) -> Measurement {
    let mut m = Measurement::default();
    match ents.len() {
        0 => {}
        1 => {
            let e = &ents[0];
            m.rows = e.rows();
            m.label = e.label_pos();
            let radius_txt = |unit_kind: &str, value: f64, unit: &str| -> String {
                let base = format!("{}: {} {} {}", e.type_name, unit_kind, fmt_len(value), unit);
                match e.radius {
                    Some(r) if r != 0.0 => {
                        format!("{base}, Ø {} / R {} mm", fmt_len(2.0 * r), fmt_len(r))
                    }
                    _ => base,
                }
            };
            match (e.kind, e.length, e.area) {
                (MeasureKind::Edge, Some(l), _) => m.status = Some(radius_txt("délka", l, "mm")),
                (MeasureKind::Face, _, Some(a)) => m.status = Some(radius_txt("plocha", a, "mm²")),
                _ => {}
            }
        }
        2 => {
            let (rel, draw) = relation(&ents[0], &ents[1], dist);
            for (i, e) in ents.iter().enumerate() {
                let key = match e.kind {
                    MeasureKind::Edge => e.length.map(|l| ("délka", l)),
                    MeasureKind::Face => e.area.map(|a| ("plocha", a)),
                    _ => None,
                };
                let mut desc = format!("{} – {}", e.kind_cz(), e.type_name);
                if let Some(r) = e.radius {
                    desc.push_str(&format!(", Ø {}", fmt_len(2.0 * r)));
                }
                if let Some((name, v)) = key {
                    desc.push_str(&format!(", {} {}", name, fmt_len(v)));
                }
                m.rows
                    .push(MeasureRow::new(format!("Entita {}", i + 1), desc));
            }
            m.rows.extend(rel);
            if let Some((p1, p2)) = draw.min {
                m.distance_label = Some(format!("{:.3} mm", distance(&p1, &p2)).replace('.', ","));
                let dv = sub(&p2, &p1);
                m.status = Some(format!(
                    "Vzdálenost {} mm  (ΔX {}, ΔY {}, ΔZ {})",
                    fmt_len(distance(&p1, &p2)),
                    fmt_len(dv[0]),
                    fmt_len(dv[1]),
                    fmt_len(dv[2])
                ));
            }
            m.min_points = draw.min;
            m.center_points = draw.center;
        }
        _ => {
            m.rows = summary(&ents);
            for (i, e) in ents.iter().take(12).enumerate() {
                m.rows.push(MeasureRow::new(
                    format!("{}.", i + 1),
                    format!("{} – {}", e.kind_cz(), e.type_name),
                ));
            }
        }
    }
    m.entities = ents;
    m
}

pub(crate) fn kind_code(k: EntityKind) -> u8 {
    match k {
        EntityKind::Vertex => 0,
        EntityKind::Edge => 1,
        EntityKind::Face => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plane(normal: P3, point: P3, reversed_label: bool) -> Entity {
        let mut e = Entity::empty(MeasureKind::Face, "rovina");
        e.axis = Some(normal);
        e.center = Some(point);
        e.plane_point = Some(point);
        e.area = Some(100.0);
        e.centroid = Some(point);
        let _ = reversed_label;
        e
    }

    fn cylinder(center: P3, axis: P3, r: f64) -> Entity {
        let mut e = Entity::empty(MeasureKind::Face, "válec");
        e.axis = Some(axis);
        e.center = Some(center);
        e.radius = Some(r);
        e.is_line = true;
        e.area = Some(50.0);
        e.centroid = Some(center);
        e
    }

    #[test]
    fn number_format_matches_python() {
        assert_eq!(fmt_p(1234567.891, 3), "1 234 567,891");
        assert_eq!(fmt_p(-1234.5, 1), "-1 234,5");
        assert_eq!(fmt_p(0.0, 3), "0,000");
        assert_eq!(fmt_p(999.9996, 3), "1 000,000");
        assert_eq!(fmt_p(12.0, 0), "12");
        assert_eq!(fmt_p(-0.0004, 3), "-0,000");
        // ties follow the exact binary value (round half to even), like Python's format()
        assert_eq!(fmt_p(0.125, 2), "0,12");
        assert_eq!(fmt_p(0.375, 2), "0,38");
        assert_eq!(fmt_p(2.5, 0), "2");
        assert_eq!(fmt_len(1e6), "1 000 000,00");
        assert_eq!(fmt_thousands(300_000), "300 000");
        assert_eq!(fmt_thousands(999), "999");
        assert_eq!(fmt_thousands(1_234_567), "1 234 567");
        assert_eq!(fmt_dir(&[0.0, 0.0, 1.0]), "(0,0000; 0,0000; 1,0000)");
        assert_eq!(fmt_pt(&[1.0, -2.5, 1000.0]), "1,000; -2,500; 1 000,000");
    }

    #[test]
    fn direction_angle_matches_gp_dir() {
        assert!(
            (dir_angle(&[1.0, 0.0, 0.0], &[0.0, 1.0, 0.0]) - std::f64::consts::FRAC_PI_2).abs()
                < 1e-15
        );
        assert!(dir_angle(&[1.0, 0.0, 0.0], &[1.0, 0.0, 0.0]).abs() < 1e-15);
        assert!(
            (dir_angle(&[1.0, 0.0, 0.0], &[-1.0, 0.0, 0.0]) - std::f64::consts::PI).abs() < 1e-15
        );
        // asin branch keeps precision for tiny angles
        let tiny: f64 = 1e-9;
        let a = dir_angle(&[1.0, 0.0, 0.0], &[tiny.cos(), tiny.sin(), 0.0]);
        assert!((a - tiny).abs() < 1e-18);
    }

    #[test]
    fn parallel_planes_give_plane_distance() {
        let a = plane([0.0, 0.0, 1.0], [0.0, 0.0, 0.0], false);
        let b = plane([0.0, 0.0, -1.0], [5.0, 5.0, 10.0], true);
        let (rows, draw) = relation(&a, &b, None);
        assert!(draw.min.is_none() && draw.center.is_none());
        assert_eq!(rows[0], MeasureRow::new("Úhel", "180,000°"));
        assert_eq!(rows[1], MeasureRow::bold("Vzdálenost rovin", "10,000 mm"));
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn plane_tolerance_is_1e4_degrees_for_two_planes() {
        let a = plane([0.0, 0.0, 1.0], [0.0, 0.0, 0.0], false);
        // 0.001 degrees apart: not parallel for B-rep planes
        let t = 0.001_f64.to_radians();
        let b = plane([t.sin(), 0.0, t.cos()], [0.0, 0.0, 10.0], false);
        let (rows, _) = relation(&a, &b, None);
        assert_eq!(rows.len(), 1, "{rows:?}");
        // a picked point with a normal acts as a plane with 2 degrees of tolerance and the thickness label
        let mut p = Entity::picked_point(
            [0.0, 0.0, 10.0],
            Some([t.sin(), 0.0, t.cos()]),
            false,
            false,
            None,
        )
        .unwrap();
        p.type_name = "bod na ploše".to_string();
        let (rows, _) = relation(&a, &p, None);
        assert_eq!(
            rows[1],
            MeasureRow::bold("Kolmá vzdálenost (tloušťka)", "10,000 mm")
        );
        // 2.5 degrees is beyond the 2 degree tolerance
        let t = 2.5_f64.to_radians();
        let q = Entity::picked_point(
            [0.0, 0.0, 10.0],
            Some([t.sin(), 0.0, t.cos()]),
            false,
            false,
            None,
        )
        .unwrap();
        let (rows, _) = relation(&a, &q, None);
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn line_and_plane_angle() {
        let p = plane([0.0, 0.0, 1.0], [0.0, 0.0, 0.0], false);
        let c = cylinder(
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 1.0].map(|v| v / 2f64.sqrt()),
            3.0,
        );
        let (rows, _) = relation(&p, &c, None);
        // normal-axis angle is 45 degrees: line to plane = |90 - 45|
        assert_eq!(
            rows[0],
            MeasureRow::new("Úhel (přímka – rovina)", "45,000°")
        );
    }

    #[test]
    fn axis_distance_parallel_and_skew() {
        let a = cylinder([0.0, 0.0, 0.0], [0.0, 0.0, 1.0], 1.0);
        let b = cylinder([3.0, 4.0, 0.0], [0.0, 0.0, 1.0], 2.0);
        let (rows, _) = relation(&a, &b, None);
        assert_eq!(
            rows.last().unwrap(),
            &MeasureRow::new("Vzdálenost os (rovnoběžné)", "5,000 mm")
        );
        assert_eq!(rows[0], MeasureRow::new("Úhel", "0,000°"));
        let c = cylinder([5.0, 0.0, 2.0], [0.0, 1.0, 0.0], 1.0);
        let (rows, _) = relation(&a, &c, None);
        assert_eq!(rows[0], MeasureRow::new("Úhel", "90,000°"));
        assert_eq!(
            rows.last().unwrap(),
            &MeasureRow::new("Vzdálenost os (mimoběžné)", "5,000 mm")
        );
    }

    #[test]
    fn line_line_parallel_threshold() {
        let (d, par) =
            line_line_distance(&[0., 0., 0.], &[1., 0., 0.], &[0., 3., 4.], &[1., 0., 0.]);
        assert!(par);
        assert!((d - 5.0).abs() < 1e-12);
        let (d, par) =
            line_line_distance(&[0., 0., 0.], &[1., 0., 0.], &[0., 0., 7.], &[0., 1., 0.]);
        assert!(!par);
        assert!((d - 7.0).abs() < 1e-12);
    }

    #[test]
    fn minimum_distance_rows_and_centres() {
        let mut a = Entity::empty(MeasureKind::Edge, "kružnice");
        a.radius = Some(5.0);
        a.center = Some([0.0, 0.0, 0.0]);
        a.axis = Some([0.0, 0.0, 1.0]);
        a.length = Some(31.4159);
        a.p0 = Some([5.0, 0.0, 0.0]);
        a.p1 = Some([5.0, 0.0, 0.0]);
        let mut b = a.clone();
        b.center = Some([30.0, 40.0, 0.0]);
        let d = Distance {
            value: 2.5,
            p1: [1.0, 2.0, 3.0],
            p2: [1.0, 2.0, 5.5],
        };
        let (rows, draw) = relation(&a, &b, Some(d));
        assert_eq!(rows[0], MeasureRow::bold("Min. vzdálenost", "2,500 mm"));
        assert_eq!(rows[1].label, "ΔX / ΔY / ΔZ");
        assert_eq!(rows[1].value, "0,000 / 0,000 / 2,500 mm");
        assert_eq!(rows[2], MeasureRow::new("Bod 1", "1,000; 2,000; 3,000"));
        assert_eq!(rows[4], MeasureRow::new("Vzdálenost středů", "50,000 mm"));
        assert_eq!(rows[5].value, "30,000 / 40,000 / 0,000 mm");
        assert_eq!(draw.min, Some(([1.0, 2.0, 3.0], [1.0, 2.0, 5.5])));
        assert_eq!(draw.center, Some(([0.0, 0.0, 0.0], [30.0, 40.0, 0.0])));
        // circle axes are normals (is_line false on both): "Úhel" without the min(ang, 180 - ang) fold
        assert_eq!(rows[6], MeasureRow::new("Úhel", "0,000°"));
    }

    #[test]
    fn summary_rows() {
        let mut e1 = Entity::empty(MeasureKind::Edge, "úsečka");
        e1.length = Some(10.0);
        let mut e2 = Entity::empty(MeasureKind::Edge, "úsečka");
        e2.length = Some(2.5);
        let mut f1 = Entity::empty(MeasureKind::Face, "rovina");
        f1.area = Some(1000.0);
        let patch = MeshPatch {
            id: 7,
            facets: 4,
            area: 200.0,
            centroid: [0.0; 3],
        };
        let p1 = Entity::picked_point([0.0; 3], None, true, false, Some(patch)).unwrap();
        let p2 = Entity::picked_point([1.0; 3], None, true, false, Some(patch)).unwrap();
        let rows = summary(&[e1, e2, f1, p1, p2]);
        assert_eq!(rows[0], MeasureRow::new("Vybráno", "5 entit"));
        assert_eq!(
            rows[1],
            MeasureRow::new("Součet délek hran", "12,500 mm (2 hran)")
        );
        assert_eq!(
            rows[2],
            MeasureRow::new("Součet ploch", "1 000,00 mm²  (10,000 cm², 1 ploch)")
        );
        assert_eq!(
            rows[3],
            MeasureRow::new(
                "Součet rovinných ploch (síť)",
                "200,00 mm²  (2,000 cm², 1 ploch)"
            )
        );
    }

    #[test]
    fn entity_rows_cz() {
        let mut c = Entity::empty(MeasureKind::Edge, "oblouk");
        c.radius = Some(12.5);
        c.center = Some([1.0, 2.0, 3.0]);
        c.axis = Some([0.0, 0.0, 1.0]);
        c.length = Some(40.0);
        c.arc = Some(90.0);
        c.p0 = Some([13.5, 2.0, 3.0]);
        c.p1 = Some([1.0, 14.5, 3.0]);
        let rows = c.rows();
        let labels: Vec<&str> = rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Entita",
                "Délka",
                "Poloměr R",
                "Průměr Ø",
                "Střed",
                "Osa",
                "Úhel oblouku",
                "Začátek",
                "Konec"
            ]
        );
        assert_eq!(rows[0].value, "hrana – oblouk");
        assert_eq!(rows[6].value, "90,00°");
        assert_eq!(c.short(), "hrana: oblouk  Ø 25,000  R 12,500  L 40,000");
        let (p, txt) = c.label_pos().unwrap();
        assert_eq!(p, [13.5, 2.0, 3.0]);
        assert_eq!(txt, "Ø 25,00");

        let mut l = Entity::empty(MeasureKind::Edge, "úsečka");
        l.is_line = true;
        l.axis = Some([1.0, 0.0, 0.0]);
        l.length = Some(10.0);
        l.p0 = Some([0.0, 0.0, 0.0]);
        l.p1 = Some([10.0, 0.0, 0.0]);
        let rows = l.rows();
        assert_eq!(rows[2].label, "Směr");
        assert_eq!(
            rows[3],
            MeasureRow::new("ΔX; ΔY; ΔZ", "10,000; 0,000; 0,000")
        );
        assert_eq!(
            l.label_pos().unwrap(),
            ([5.0, 0.0, 0.0], "10,00".to_string())
        );

        let mut cone = Entity::empty(MeasureKind::Face, "kužel");
        cone.axis = Some([0.0, 0.0, 1.0]);
        cone.half_angle = Some(15.0);
        cone.area = Some(314.159);
        cone.centroid = Some([0.0, 0.0, 5.0]);
        let rows = cone.rows();
        let labels: Vec<&str> = rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Entita",
                "Plocha",
                "Osa",
                "Poloviční úhel",
                "Těžiště plochy"
            ]
        );
        assert_eq!(rows[1].value, "314,16 mm²  (3,142 cm²)");
        assert_eq!(cone.short(), "plocha: kužel  A 314,16 mm²");
    }

    #[test]
    fn mesh_point_and_loop_entities() {
        let patch = MeshPatch {
            id: 1,
            facets: 12,
            area: 250.0,
            centroid: [1.0, 2.0, 3.0],
        };
        let p = Entity::picked_point(
            [0.0, 0.0, 1.0],
            Some([0.0, 0.0, 2.0]),
            true,
            true,
            Some(patch),
        )
        .unwrap();
        assert_eq!(p.type_name, "vrchol sítě, rovina");
        assert_eq!(p.axis, Some([0.0, 0.0, 1.0]));
        let rows = p.rows();
        assert_eq!(rows[0].value, "bod – vrchol sítě, rovina");
        assert_eq!(
            rows[2],
            MeasureRow::new("Normála", "(0,0000; 0,0000; 1,0000)")
        );
        assert_eq!(
            rows[3],
            MeasureRow::new(
                "Rovinná plocha (síť)",
                "250,00 mm²  (2,500 cm², 12 trojúh.)"
            )
        );
        assert_eq!(
            p.label_pos().unwrap(),
            ([1.0, 2.0, 3.0], "250,0 mm²".to_string())
        );
        assert_eq!(
            p.short(),
            "bod: vrchol sítě, rovina  (0,000; 0,000; 1,000)  A 250,00 mm²"
        );
        assert!(Entity::picked_point([0.0; 3], Some([0.0; 3]), false, false, None).is_err());

        let lp = MeshLoop {
            points: vec![[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]],
            closed: true,
            length: std::f64::consts::TAU,
            kind: "kružnice".to_string(),
            circle: Some(LoopCircle {
                center: [0.0; 3],
                normal: [0.0, 0.0, 3.0],
                radius: 1.0,
                rms: 0.002,
            }),
            coverage: 360.0,
        };
        let e = Entity::mesh_loop(&lp).unwrap();
        assert_eq!(e.type_name, "kružnice (síť)");
        let rows = e.rows();
        assert_eq!(rows.last().unwrap().label, "Konec");
        assert!(
            rows.iter()
                .any(|r| r.label == "Proložení kružnice" && r.value == "± 0,002 mm (ze sítě)")
        );
        assert!(!rows.iter().any(|r| r.label == "Úhel oblouku"));
        let arc = MeshLoop {
            kind: "oblouk".to_string(),
            coverage: 120.0,
            ..lp
        };
        let e = Entity::mesh_loop(&arc).unwrap();
        assert!(
            e.rows()
                .iter()
                .any(|r| r.label == "Úhel oblouku" && r.value == "120,00°")
        );
    }

    #[test]
    fn assembled_panel_for_one_two_and_many() {
        let mut c = Entity::empty(MeasureKind::Edge, "kružnice");
        c.radius = Some(5.0);
        c.length = Some(31.42);
        c.center = Some([0.0; 3]);
        c.axis = Some([0.0, 0.0, 1.0]);
        c.p0 = Some([5.0, 0.0, 0.0]);
        c.p1 = Some([5.0, 0.0, 0.0]);
        c.arc = Some(360.0);
        let m = assemble(vec![c.clone()], None);
        assert_eq!(
            m.status.as_deref(),
            Some("kružnice: délka 31,42 mm, Ø 10,00 / R 5,00 mm")
        );
        assert_eq!(m.label.as_ref().map(|l| l.1.as_str()), Some("Ø 10,00"));

        let mut f = Entity::empty(MeasureKind::Face, "rovina");
        f.area = Some(2500.0);
        f.centroid = Some([0.0; 3]);
        let m = assemble(vec![f.clone()], None);
        assert_eq!(m.status.as_deref(), Some("rovina: plocha 2 500,00 mm²"));

        let d = Distance {
            value: 5.0,
            p1: [0.0, 0.0, 0.0],
            p2: [3.0, 4.0, 0.0],
        };
        let m = assemble(vec![c.clone(), f.clone()], Some(d));
        assert_eq!(
            m.rows[0],
            MeasureRow::new("Entita 1", "hrana – kružnice, Ø 10,00, délka 31,42")
        );
        assert_eq!(
            m.rows[1],
            MeasureRow::new("Entita 2", "plocha – rovina, plocha 2 500,00")
        );
        assert_eq!(m.distance_label.as_deref(), Some("5,000 mm"));
        assert_eq!(
            m.status.as_deref(),
            Some("Vzdálenost 5,00 mm  (ΔX 3,00, ΔY 4,00, ΔZ 0,00)")
        );
        assert_eq!(m.min_points, Some(([0.0; 3], [3.0, 4.0, 0.0])));

        let many: Vec<Entity> = (0..14).map(|_| f.clone()).collect();
        let m = assemble(many, None);
        // summary (2 rows) + 12 listed entities
        assert_eq!(m.rows.len(), 2 + 12);
        assert_eq!(m.rows[2], MeasureRow::new("1.", "plocha – rovina"));
        assert_eq!(m.rows[0], MeasureRow::new("Vybráno", "14 entit"));
    }

    #[test]
    fn failed_measurement_row() {
        let m = Measurement::failed("něco se pokazilo");
        assert_eq!(
            m.rows,
            vec![MeasureRow::new(
                "Měření",
                "nelze vyhodnotit: něco se pokazilo"
            )]
        );
        assert!(m.error.is_some());
    }

    #[test]
    fn ref_points() {
        let mut s = Entity::empty(MeasureKind::Face, "koule");
        s.center = Some([1.0, 2.0, 3.0]);
        s.radius = Some(4.0);
        assert_eq!(s.ref_point(), Some([1.0, 2.0, 3.0]));
        let cy = cylinder([1.0, 1.0, 1.0], [0.0, 0.0, 1.0], 2.0);
        assert_eq!(cy.ref_point(), None);
    }
}
