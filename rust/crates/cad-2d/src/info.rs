//! Drawing information, entity property rows and security checks of the 2D viewer
//! (`view2d.doc_info`, `entity_rows`, `layer_table`, `is_remote_path`, `remote_images`).
use crate::colors::*;
use crate::dxf::*;
use crate::frontend::*;
use crate::geom::*;
use crate::mtext::plain_mtext;

pub const NOT_DRAWABLE: [&str; 11] = [
    "3DSOLID",
    "BODY",
    "REGION",
    "SURFACE",
    "EXTRUDEDSURFACE",
    "LOFTEDSURFACE",
    "REVOLVEDSURFACE",
    "SWEPTSURFACE",
    "PLANESURFACE",
    "VIEWPORT",
    "ACAD_PROXY_ENTITY",
];

/// ACIS solids: counted for the warning "N× 3D těleso (ACIS)".
pub const ACIS_TYPES: [&str; 4] = ["3DSOLID", "BODY", "REGION", "SURFACE"];

/// `$INSUNITS` code to unit name.
pub fn units_name(code: i32) -> String {
    match code {
        0 => "bez jednotek".into(),
        1 => "palce".into(),
        2 => "stopy".into(),
        4 => "mm".into(),
        5 => "cm".into(),
        6 => "m".into(),
        7 => "km".into(),
        8 => "mikropalce".into(),
        9 => "mil".into(),
        10 => "yardy".into(),
        13 => "µm".into(),
        14 => "dm".into(),
        other => other.to_string(),
    }
}

/// AutoCAD release name of a DXF version code (ezdxf `acad_release`).
pub fn release_name(version: &str) -> String {
    match version {
        "AC1006" => "R10",
        "AC1009" => "R12",
        "AC1012" => "R13",
        "AC1014" => "R14",
        "AC1015" => "R2000",
        "AC1018" => "R2004",
        "AC1021" => "R2007",
        "AC1024" => "R2010",
        "AC1027" => "R2013",
        "AC1032" => "R2018",
        other => other,
    }
    .to_string()
}

/// The version ezdxf reports after loading: R10 / R12 files are R12, R13 / R14 are upgraded to R2000.
pub fn loaded_version(code: &str) -> String {
    match code {
        "" | "AC1006" | "AC1009" => "AC1009".into(),
        "AC1012" | "AC1014" => "AC1015".into(),
        other => other.to_string(),
    }
}

/// `f"{v:,.{nd}f}"` with a space as thousands separator and a comma as decimal separator.
pub fn fmt_p(v: f64, nd: usize) -> String {
    if !v.is_finite() {
        return v.to_string();
    }
    let s = format!("{:.*}", nd, v);
    let (neg, s) = match s.strip_prefix('-') {
        Some(r) => (true, r.to_string()),
        None => (false, s),
    };
    let (int, frac) = match s.split_once('.') {
        Some((i, f)) => (i.to_string(), Some(f.to_string())),
        None => (s, None),
    };
    let mut out = String::new();
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            out.push(' ');
        }
        out.push(c);
    }
    let mut r = out;
    if let Some(f) = frac {
        r.push(',');
        r.push_str(&f);
    }
    // a negative zero ("-0,00") keeps its sign in Python as well
    if neg {
        format!("-{}", r)
    } else {
        r
    }
}

fn fmt_pt(p: V3) -> String {
    format!("{}, {}", fmt_p(p[0], 2), fmt_p(p[1], 2))
}

/// A path is a URL, UNC path, device / NT path: opening it would make Windows connect to a server.
pub fn is_remote_path(name: &str) -> bool {
    let raw: String = name.chars().filter(|&c| c >= ' ').collect::<String>().trim().to_string();
    let n = raw.replace('/', "\\");
    if n.is_empty() {
        return false;
    }
    if n.starts_with("\\\\") || n.starts_with("\\??\\") || raw.contains("://") {
        return true;
    }
    // URL scheme: a letter followed by one or more of [A-Za-z0-9+.-] and a colon ("C:" is a drive, "file:" an URL)
    let mut it = n.chars();
    if let Some(first) = it.next() {
        if first.is_ascii_alphabetic() {
            let mut k = 0;
            for c in it {
                if c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-') {
                    k += 1;
                } else {
                    return k >= 1 && c == ':';
                }
            }
        }
    }
    false
}

/// IMAGEDEF file names that are not plain local paths.
pub fn remote_images(doc: &Document) -> Vec<String> {
    let mut v: Vec<String> = doc.image_defs.values().filter(|d| is_remote_path(&d.filename)).map(|d| d.filename.clone()).collect();
    v.sort();
    v
}

pub fn is_drawable(e: &Entity) -> bool {
    !NOT_DRAWABLE.contains(&e.dxftype())
}

/// Layer table: (name, colour, visible in the file).
pub fn layer_table(doc: &Document, light_bg: bool) -> Vec<(String, [u8; 3], bool)> {
    let fg = if light_bg { [0u8, 0, 0] } else { [255u8, 255, 255] };
    let _ = fg;
    doc.layers
        .iter()
        .map(|l| {
            let rgb = match l.true_color {
                Some(tc) => rgb_from_u32(tc),
                None => {
                    let aci = l.color.abs();
                    aci_rgb(if aci == 0 { 7 } else { aci })
                }
            };
            (l.name.clone(), rgb, l.is_on() && !l.is_frozen())
        })
        .collect()
}

/// (types, counts) most common first, ties in order of first appearance.
pub fn type_counts(ents: &[Entity]) -> Vec<(String, usize)> {
    let mut v: Vec<(String, usize)> = Vec::new();
    let mut index: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for e in ents {
        let t = e.dxftype();
        match index.get(t) {
            Some(&i) => v[i].1 += 1,
            None => {
                index.insert(t, v.len());
                v.push((t.to_string(), 1));
            }
        }
    }
    v.sort_by(|a, b| b.1.cmp(&a.1));
    v
}

pub fn types_text(counts: &[(String, usize)]) -> String {
    counts.iter().take(8).map(|(t, n)| format!("{} {}", t, n)).collect::<Vec<_>>().join(", ")
}

/// Entity rows of the properties panel (`view2d.entity_rows`). `xf` places entities of a block reference.
pub fn entity_rows(doc: &Document, e: &Entity, xf: Option<&Xform>) -> Vec<(String, String)> {
    let t = e.dxftype().to_string();
    let layer = doc.layer_names.name(e.c.layer).to_string();
    let mut rows: Vec<(String, String)> = vec![("Entita".into(), t.clone()), ("Hladina".into(), layer)];
    let ident = Xform::IDENTITY;
    let x = xf.unwrap_or(&ident);
    let scale = x.uniform_scale_2d().unwrap_or(1.0);
    let rot_deg = x.m[1][0].atan2(x.m[0][0]).to_degrees();
    let mirrored = x.det() < 0.0;
    let mut push = |k: &str, v: String| rows.push((k.to_string(), v));
    match &e.d {
        Data::Line { start, end } => {
            let (s, en) = (x.apply(*start), x.apply(*end));
            push("Délka", fmt_p(dist(s, en), 2));
            push("Úhel", format!("{}°", fmt_p(fmod((en[1] - s[1]).atan2(en[0] - s[0]).to_degrees(), 360.0), 2)));
            push("Začátek", fmt_pt(s));
            push("Konec", fmt_pt(en));
        }
        Data::Circle { center, radius } => {
            let r = radius * scale;
            push("Poloměr", fmt_p(r, 2));
            push("Průměr", fmt_p(2.0 * r, 2));
            push("Střed", fmt_pt(x.apply(*center)));
            push("Obvod", fmt_p(2.0 * std::f64::consts::PI * r, 2));
        }
        Data::Arc { center, radius, start, end } => {
            let r = radius * scale;
            let a = fmod(end - start, 360.0);
            push("Poloměr", fmt_p(r, 2));
            push("Průměr", fmt_p(2.0 * r, 2));
            push("Střed", fmt_pt(x.apply(*center)));
            push("Úhel oblouku", format!("{}°", fmt_p(a, 2)));
            push("Délka oblouku", fmt_p(a.to_radians() * r, 2));
        }
        Data::LwPolyline(_) | Data::Polyline(_) | Data::Spline(_) | Data::Ellipse { .. } => {
            if let Some(path) = entity_path(e) {
                let pts = path.flatten(0.01, 4);
                let ln: f64 = pts.windows(2).map(|w| dist(x.apply(w[0]), x.apply(w[1]))).sum();
                push("Délka", fmt_p(ln, 2));
            }
            if let Data::LwPolyline(p) = &e.d {
                push("Vrcholů", p.points.len().to_string());
                push("Uzavřená", if p.closed { "ano" } else { "ne" }.to_string());
            }
        }
        Data::Text(tx) => {
            let raw = if tx.kind == TextKind::Attdef { &tx.tag } else { &tx.text };
            push("Text", raw.clone());
            push("Výška", fmt_p(tx.height * scale, 2));
        }
        Data::MText(m) => {
            let p: String = plain_mtext(&m.text).chars().take(300).collect();
            push("Text", p);
            push("Výška", fmt_p(m.char_height * scale, 2));
        }
        Data::Dimension(d) => {
            let val = dimension_measurement(d);
            push("Naměřeno", fmt_p(val, 3));
            if !d.text.is_empty() {
                push("Text kóty", d.text.clone());
            }
        }
        Data::Insert(i) if !i.from_inventor => {
            push("Blok", i.name.clone());
            push("Vložení", fmt_pt(x.apply(i.insert)));
            push("Měřítko", format!("{} / {}", fmt_p(i.scale[0] * scale, 3), fmt_p(i.scale[1] * scale * if mirrored { 1.0 } else { 1.0 }, 3)));
            push("Natočení", format!("{}°", fmt_p(i.rotation + rot_deg, 2)));
        }
        Data::Hatch(h) => push("Šrafa", h.pattern_name.clone()),
        _ => {}
    }
    if e.c.handle != 0 {
        rows.push(("Handle".into(), format!("{:X}", e.c.handle)));
    }
    rows
}

/// Measured value of a dimension (`Dimension.get_measurement` for the common types).
pub fn dimension_measurement(d: &DimData) -> f64 {
    if let Some(m) = d.actual_measurement {
        return m;
    }
    match d.dimtype & 7 {
        0 => {
            let a = d.angle.to_radians();
            let v = sub(d.defpoint3, d.defpoint2);
            (v[0] * a.cos() + v[1] * a.sin()).abs()
        }
        1 => dist(d.defpoint2, d.defpoint3),
        3 | 4 => dist(d.defpoint, d.defpoint4),
        _ => 0.0,
    }
}

/// Path of a curve-like entity (for lengths).
pub fn entity_path(e: &Entity) -> Option<Path> {
    let ocs = Ocs::new(e.c.extrusion);
    match &e.d {
        Data::LwPolyline(p) => {
            let pts: Vec<(f64, f64, f64)> = p.points.iter().map(|v| (v.x, v.y, v.bulge)).collect();
            Some(polyline_path(&pts, p.closed, &ocs, p.elevation))
        }
        Data::Polyline(p) => {
            if p.vertices.is_empty() {
                return None;
            }
            if p.is_3d() {
                let pts: Vec<V3> = p.vertices.iter().map(|v| v.loc).collect();
                return Some(path_from_vertices(&pts, p.is_closed()));
            }
            let pts: Vec<(f64, f64, f64)> = p.vertices.iter().map(|v| (v.loc[0], v.loc[1], v.bulge)).collect();
            let elev = if p.has_elevation { p.elevation[2] } else { p.vertices[0].loc[2] };
            Some(polyline_path(&pts, p.is_closed(), &ocs, elev))
        }
        Data::Spline(s) => spline_path(s),
        Data::Ellipse { center, major, ratio, start, end } => ellipse_path(*center, *major, e.c.extrusion, *ratio, *start, *end),
        _ => None,
    }
}

/// Rows of the layer properties panel (layer selected in the list).
pub fn layer_rows(doc: &Document, ents: &[Entity], name_key: &str) -> Vec<(String, String)> {
    let key = name_key.to_lowercase();
    let on_layer: Vec<&Entity> = ents.iter().filter(|e| doc.layer_names.name(e.c.layer).to_lowercase() == key).collect();
    let lay = doc.layers.iter().find(|l| l.name.to_lowercase() == key);
    let mut rows = vec![
        ("Hladina".to_string(), lay.map(|l| l.name.clone()).unwrap_or_else(|| name_key.to_string())),
        ("Entit v layoutu".to_string(), on_layer.len().to_string()),
    ];
    if let Some(l) = lay {
        rows.push(("Barva (ACI)".into(), l.color.abs().to_string()));
        rows.push(("Typ čáry".into(), doc.linetype_names.name(l.linetype).to_string()));
        let mut st = String::from(if l.is_on() { "zapnutá" } else { "vypnutá" });
        if l.is_frozen() {
            st.push_str(", zmrazená");
        }
        if l.is_locked() {
            st.push_str(", zamčená");
        }
        rows.push(("Stav ve výkresu".into(), st));
    }
    let mut counts: Vec<(String, usize)> = Vec::new();
    for e in &on_layer {
        let t = e.dxftype();
        match counts.iter_mut().find(|(n, _)| n == t) {
            Some((_, c)) => *c += 1,
            None => counts.push((t.to_string(), 1)),
        }
    }
    counts.sort_by(|a, b| b.1.cmp(&a.1));
    if !counts.is_empty() {
        rows.push(("Typy".into(), types_text(&counts)));
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn number_format_matches_python() {
        assert_eq!(fmt_p(1234567.891, 2), "1 234 567,89");
        assert_eq!(fmt_p(-0.5, 2), "-0,50");
        assert_eq!(fmt_p(12.0, 0), "12");
        assert_eq!(fmt_p(1000.0, 3), "1 000,000");
        assert_eq!(fmt_p(999.999, 2), "1 000,00");
    }

    #[test]
    fn remote_path_detection() {
        assert!(is_remote_path("\\\\server\\share\\a.png"));
        assert!(is_remote_path("//server/share/a.png"));
        assert!(is_remote_path("http://x/y.png"));
        assert!(is_remote_path("file:C:/x.png"));
        assert!(is_remote_path("\\\\?\\C:\\x.png"));
        assert!(is_remote_path("  \\\\host\\s  "));
        assert!(!is_remote_path("C:\\images\\a.png"));
        assert!(!is_remote_path("images/a.png"));
        assert!(!is_remote_path("a.png"));
        assert!(!is_remote_path(""));
    }

    #[test]
    fn release_names() {
        assert_eq!(release_name("AC1027"), "R2013");
        assert_eq!(loaded_version("AC1014"), "AC1015");
        assert_eq!(units_name(4), "mm");
        assert_eq!(units_name(99), "99");
    }
}
