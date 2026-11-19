//! Mass, volume and centre of gravity of a set of leaf instances (`main.mass_props`, `_mass_rows`) and the panel
//! rows built from them.
use std::collections::BTreeSet;

use cad_core::fmt::{fmt_len, fmt_num};
use cad_types::{Mat4, NodeId, P3};

use crate::props::{LinkTarget, PropRow, Span};

/// Heading of the panel of `show_mass` (its first row).
pub const HEADING: &str = "Hmotnostní vlastnosti";

/// Steel and aluminium densities used for the "no material" estimate (`main.py:1587-1588`), g/cm³.
pub const STEEL_DENSITY: f64 = 7.85;
pub const ALUMINIUM_DENSITY: f64 = 2.70;

/// One leaf instance: the mass properties of its prototype placed in the world, and its material.
#[derive(Clone, Debug, PartialEq)]
pub struct LeafMass {
    pub volume_mm3: f64,
    pub area_mm2: f64,
    /// Centroid of the volume in world coordinates.
    pub centroid: P3,
    pub solids: u32,
    pub faces: u32,
    /// `(name, density g/cm³)` of the assigned (or default) material.
    pub material: Option<(String, f64)>,
}

/// Totals over leaf instances (the dictionary `d` of `mass_props`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Totals {
    pub volume: f64,
    pub area: f64,
    pub solids: u32,
    pub faces: u32,
    /// kg of the parts that have a material.
    pub mass: f64,
    /// Parts without a material.
    pub missing: u32,
    pub mats: BTreeSet<String>,
    /// Centre of gravity weighted by mass; `None` when no part has a material.
    pub cog: Option<P3>,
    /// Centre of the volume (origin when there is no volume).
    pub vcog: P3,
}

/// A point transformed by a column-major 4x4 matrix.
pub fn transform_point(m: &Mat4, p: P3) -> P3 {
    [
        m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12],
        m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13],
        m[2] * p[0] + m[6] * p[1] + m[10] * p[2] + m[14],
    ]
}

/// `kg = volume mm³ × density g/cm³ × 1e-6`.
pub fn kilograms(volume_mm3: f64, density: f64) -> f64 {
    volume_mm3 * density * 1e-6
}

pub fn totals(leaves: &[LeafMass]) -> Totals {
    let mut d = Totals::default();
    let (mut m, mut v) = ([0.0f64; 3], [0.0f64; 3]);
    for lf in leaves {
        d.volume += lf.volume_mm3;
        d.area += lf.area_mm2;
        d.solids += lf.solids;
        d.faces += lf.faces;
        for k in 0..3 {
            v[k] += lf.volume_mm3 * lf.centroid[k];
        }
        match &lf.material {
            None => d.missing += 1,
            Some((name, density)) => {
                let kg = kilograms(lf.volume_mm3, *density);
                d.mass += kg;
                d.mats.insert(name.clone());
                for k in 0..3 {
                    m[k] += kg * lf.centroid[k];
                }
            }
        }
    }
    d.cog = (d.mass > 0.0).then(|| [m[0] / d.mass, m[1] / d.mass, m[2] / d.mass]);
    d.vcog = if d.volume > 0.0 { [v[0] / d.volume, v[1] / d.volume, v[2] / d.volume] } else { [0.0; 3] };
    d
}

fn point_text(p: P3) -> String {
    format!("{}; {}; {} mm", fmt_len(p[0]), fmt_len(p[1]), fmt_len(p[2]))
}

/// The rows of `_mass_rows`. With `node` the material rows get a "přiřadit…" link to that node.
pub fn rows(d: &Totals, node: Option<NodeId>) -> Vec<PropRow> {
    let mut rows = vec![
        PropRow::text("Objem", format!("{} cm³", fmt_num(d.volume / 1000.0, 3))),
        PropRow::text("Povrch", format!("≈ {} cm²", fmt_num(d.area / 100.0, 2))),
    ];
    let link = |spans: &mut Vec<Span>| {
        if let Some(n) = node {
            spans.push(Span::Text(" (".into()));
            spans.push(Span::Link("přiřadit…".into(), LinkTarget::Material(n)));
            spans.push(Span::Text(")".into()));
        }
    };
    if !d.mats.is_empty() {
        let mut spans = vec![Span::Text(d.mats.iter().cloned().collect::<Vec<_>>().join(", "))];
        link(&mut spans);
        rows.push(PropRow::spans("Materiál", spans));
    }
    if d.mass > 0.0 {
        rows.push(PropRow::bold("Hmotnost", format!("{} kg", fmt_num(d.mass, 4))));
        if let Some(c) = d.cog {
            rows.push(PropRow::text("Těžiště X; Y; Z", point_text(c)));
        }
    }
    if d.missing > 0 {
        let msg = if d.mass > 0.0 { format!("{} dílů bez materiálu", d.missing) } else { "bez materiálu".to_string() };
        let mut spans = vec![Span::Text(msg)];
        link(&mut spans);
        rows.push(PropRow::spans(if d.mats.is_empty() { "Materiál" } else { "Pozor" }, spans));
        if d.mass == 0.0 {
            rows.push(PropRow::text(
                "Hmotnost (ocel 7,85)",
                format!("{} kg", fmt_num(d.volume * STEEL_DENSITY * 1e-6, 3)),
            ));
            rows.push(PropRow::text(
                "Hmotnost (Al 2,70)",
                format!("{} kg", fmt_num(d.volume * ALUMINIUM_DENSITY * 1e-6, 3)),
            ));
        }
    }
    if d.volume > 0.0 && d.mass == 0.0 {
        rows.push(PropRow::text("Těžiště objemu", point_text(d.vcog)));
    }
    rows.push(PropRow::text("Těles / ploch", format!("{} / {}", d.solids, d.faces)));
    rows
}

/// Text of the material column in the tree (`update_tree_materials`): `×N` and the short material name, lower case
/// with a star when it came from the default.
pub fn tree_info(count: u32, material: Option<(&str, bool)>) -> String {
    let mut txt = if count > 1 { format!("×{count}") } else { String::new() };
    if let Some((name, explicit)) = material {
        let short = name.split(" (").next().unwrap_or(name).split(" /").next().unwrap_or(name);
        let short = if explicit { short.to_string() } else { format!("{}*", short.to_lowercase()) };
        txt = if txt.is_empty() { short } else { format!("{txt} · {short}") };
    }
    txt
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(vol: f64, at: P3, mat: Option<(&str, f64)>) -> LeafMass {
        LeafMass {
            volume_mm3: vol,
            area_mm2: 600.0,
            centroid: at,
            solids: 1,
            faces: 6,
            material: mat.map(|(n, d)| (n.to_string(), d)),
        }
    }

    fn label_value(rows: &[PropRow], label: &str) -> Option<String> {
        rows.iter().find(|r| r.label == label).map(PropRow::value_text)
    }

    #[test]
    fn mass_is_volume_times_density_in_kg() {
        // a 100 mm cube of steel: 1 000 000 mm³ = 1000 cm³ x 7.85 g/cm³ = 7.85 kg
        assert!((kilograms(1_000_000.0, 7.85) - 7.85).abs() < 1e-12);
        let d = totals(&[leaf(1_000_000.0, [50.0, 50.0, 50.0], Some(("Ocel (konstrukční)", 7.85)))]);
        assert!((d.mass - 7.85).abs() < 1e-12);
        assert_eq!(d.cog, Some([50.0, 50.0, 50.0]));
        assert_eq!(d.missing, 0);
        assert_eq!((d.solids, d.faces), (1, 6));
    }

    #[test]
    fn centre_of_gravity_is_weighted_by_mass_not_volume() {
        // aluminium cube at x = 0, steel cube of the same volume at x = 100: cog moves towards steel
        let d = totals(&[
            leaf(1000.0, [0.0, 0.0, 0.0], Some(("Hliník", 2.70))),
            leaf(1000.0, [100.0, 0.0, 0.0], Some(("Ocel", 7.85))),
            leaf(2000.0, [10.0, 20.0, 30.0], None),
        ]);
        let c = d.cog.unwrap();
        let expect = 100.0 * 7.85 / (2.70 + 7.85);
        assert!((c[0] - expect).abs() < 1e-9, "{c:?}");
        assert_eq!(d.missing, 1);
        // the volume centre includes the part without a material
        let vx = (0.0 * 1000.0 + 100.0 * 1000.0 + 10.0 * 2000.0) / 4000.0;
        assert!((d.vcog[0] - vx).abs() < 1e-9);
        assert_eq!(d.mats.len(), 2);
    }

    #[test]
    fn no_material_gives_estimates_and_volume_centre() {
        let d = totals(&[leaf(2_000_000.0, [1.0, 2.0, 3.0], None)]);
        assert_eq!(d.cog, None);
        let r = rows(&d, Some(7));
        assert_eq!(label_value(&r, "Objem").unwrap(), "2 000,000 cm³");
        assert_eq!(label_value(&r, "Povrch").unwrap(), "≈ 6,00 cm²");
        assert_eq!(label_value(&r, "Materiál").unwrap(), "bez materiálu (přiřadit…)");
        assert_eq!(label_value(&r, "Hmotnost (ocel 7,85)").unwrap(), "15,700 kg");
        assert_eq!(label_value(&r, "Hmotnost (Al 2,70)").unwrap(), "5,400 kg");
        assert_eq!(label_value(&r, "Těžiště objemu").unwrap(), "1,00; 2,00; 3,00 mm");
        assert_eq!(label_value(&r, "Těles / ploch").unwrap(), "1 / 6");
        assert!(label_value(&r, "Hmotnost").is_none());
    }

    #[test]
    fn rows_with_mass_and_missing_parts() {
        let d = totals(&[
            leaf(1_000_000.0, [0.0, 0.0, 0.0], Some(("Ocel (konstrukční)", 7.85))),
            leaf(1_000.0, [0.0, 0.0, 0.0], None),
        ]);
        let r = rows(&d, None);
        assert_eq!(label_value(&r, "Materiál").unwrap(), "Ocel (konstrukční)");
        assert_eq!(label_value(&r, "Hmotnost").unwrap(), "7,8500 kg");
        assert_eq!(label_value(&r, "Pozor").unwrap(), "1 dílů bez materiálu");
        assert!(label_value(&r, "Hmotnost (ocel 7,85)").is_none());
        assert!(label_value(&r, "Těžiště objemu").is_none());
        assert!(r.iter().find(|x| x.label == "Hmotnost").unwrap().value[0] == Span::Bold("7,8500 kg".into()));
    }

    #[test]
    fn transform_uses_column_major_translation() {
        let mut m = cad_types::IDENTITY;
        m[12] = 10.0;
        m[13] = -5.0;
        m[14] = 2.0;
        assert_eq!(transform_point(&m, [1.0, 2.0, 3.0]), [11.0, -3.0, 5.0]);
        // 90 degrees about Z: x -> y
        let r: Mat4 = [0., 1., 0., 0., -1., 0., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.];
        assert_eq!(transform_point(&r, [1.0, 0.0, 0.0]), [0.0, 1.0, 0.0]);
    }

    #[test]
    fn tree_column_text() {
        assert_eq!(tree_info(1, None), "");
        assert_eq!(tree_info(3, None), "×3");
        assert_eq!(tree_info(1, Some(("Ocel (konstrukční)", true))), "Ocel");
        assert_eq!(tree_info(2, Some(("Hliník (slitiny)", false))), "×2 · hliník*");
        assert_eq!(tree_info(1, Some(("PA6 / PA12", true))), "PA6");
        assert_eq!(tree_info(1, Some(("Vlastní (7,5 g/cm³)", true))), "Vlastní");
    }
}
