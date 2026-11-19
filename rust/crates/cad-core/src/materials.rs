//! Material library (densities in g/cm³) and per-file material assignments. Port of stepview/materials.py without the
//! dialog. Assignments are keyed by the prototype name (the part name), so they survive a re-export.
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::fmt::fmt_g;
use crate::paths;

/// Combo entry for a user-defined density (`CUSTOM`).
pub const CUSTOM: &str = "Vlastní hustota…";
/// Densities must lie strictly between these values (`_valid`).
pub const DENSITY_MIN_EXCL: f64 = 0.0;
pub const DENSITY_MAX_EXCL: f64 = 100.0;
/// Start value of the density spin box.
pub const DEFAULT_DENSITY: f64 = 7.85;

/// `(name, density g/cm³)`, in the order of the combo box (materials.py lines 10-18).
pub const LIBRARY: &[(&str, f64)] = &[
    ("Ocel (konstrukční)", 7.85),
    ("Nerez (1.4301)", 7.90),
    ("Litina", 7.20),
    ("Hliník (slitiny)", 2.70),
    ("Mosaz", 8.50),
    ("Měď", 8.96),
    ("Bronz", 8.80),
    ("Titan", 4.51),
    ("Zinek", 7.14),
    ("PLA", 1.24),
    ("PETG", 1.27),
    ("ABS", 1.04),
    ("ASA", 1.07),
    ("PA6 / PA12", 1.14),
    ("POM", 1.41),
    ("PTFE", 2.20),
    ("PE-HD", 0.95),
    ("PP", 0.91),
    ("PVC", 1.40),
    ("PC / plexi (PMMA)", 1.19),
    ("FR4 (DPS)", 1.85),
    ("Pryž / silikon", 1.20),
    ("Sklo", 2.50),
    ("Dřevo (smrk)", 0.45),
    ("Dřevo (buk)", 0.72),
    ("MDF / HDF", 0.75),
    ("Promasil / izolační deska", 0.24),
    ("Minerální vata", 0.10),
];

/// A density that is sane (`_valid`): a damaged file must not break the mass arithmetic.
pub fn valid_density(density: f64) -> bool {
    density > DENSITY_MIN_EXCL && density < DENSITY_MAX_EXCL
}

/// The material library (`LIB`): name to density.
#[derive(Clone, Debug, PartialEq)]
pub struct Library {
    entries: Vec<(String, f64)>,
}

impl Library {
    /// The built-in library (`LIBRARY`).
    pub fn builtin() -> Library {
        Library {
            entries: LIBRARY
                .iter()
                .map(|(n, d)| (n.to_string(), *d))
                .collect(),
        }
    }

    /// Entries in combo order.
    pub fn entries(&self) -> &[(String, f64)] {
        &self.entries
    }

    /// Density of a library entry, `None` for any other name (including `CUSTOM`).
    pub fn density(&self, name: &str) -> Option<f64> {
        self.entries.iter().find(|(n, _)| n == name).map(|(_, d)| *d)
    }

    /// Combo label of a library entry: `"<name>  –  <density> g/cm³"` with a decimal comma (`str(d)` in Python).
    pub fn combo_label(name: &str, density: f64) -> String {
        format!("{name}  –  {} g/cm³", python_float_text(density).replace('.', ","))
    }
}

/// Name stored for a user-defined density: `"Vlastní (<value:g> g/cm³)"` with a decimal comma.
pub fn custom_name(density: f64) -> String {
    format!("Vlastní ({} g/cm³)", fmt_g(density).replace('.', ","))
}

/// Python `repr` of a float for ordinary values: `7.9`, `2.0`.
fn python_float_text(d: f64) -> String {
    let s = format!("{d}");
    if d.is_finite() && !s.contains('.') && !s.contains('e') {
        format!("{s}.0")
    } else {
        s
    }
}

/// Path of the assignment file: `<cache>/materials/<sha1(normcase(abspath(model)))>.json`.
pub fn store_path(cache_dir: &Path, model: &Path) -> io::Result<PathBuf> {
    let key = paths::sha1_hex(&paths::normcase_abspath(model)?);
    Ok(cache_dir.join("materials").join(format!("{key}.json")))
}

/// A material found for a part.
#[derive(Clone, Debug, PartialEq)]
pub struct Assigned {
    pub name: String,
    pub density: f64,
    /// True when the part has its own assignment; false when it comes from the default.
    pub explicit: bool,
}

/// Material assignments of one model file (`Assignments`).
#[derive(Clone, Debug, PartialEq)]
pub struct Assignments {
    path: PathBuf,
    /// Prototype name to `(material name, density)`.
    pub by_name: BTreeMap<String, (String, f64)>,
    /// Default material for parts without one, or `None`.
    pub default: Option<(String, f64)>,
}

impl Assignments {
    /// Loads the assignments of `model`. A missing or damaged file gives empty assignments; invalid entries are
    /// dropped (as in Python).
    pub fn load(cache_dir: &Path, model: &Path) -> io::Result<Assignments> {
        let path = store_path(cache_dir, model)?;
        let mut a = Assignments {
            path,
            by_name: BTreeMap::new(),
            default: None,
        };
        let Ok(text) = fs::read_to_string(&a.path) else {
            return Ok(a);
        };
        let Ok(doc) = serde_json::from_str::<Value>(&text) else {
            return Ok(a);
        };
        if let Some(parts) = doc.get("parts").and_then(Value::as_object) {
            for (k, v) in parts {
                if let Some(e) = parse_entry(v) {
                    a.by_name.insert(k.clone(), e);
                }
            }
        }
        a.default = doc.get("default").and_then(parse_entry);
        Ok(a)
    }

    /// Where the assignments are stored.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Writes the file (`{"parts": ..., "default": ...}`, indent 1, UTF-8).
    pub fn save(&self) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let parts: serde_json::Map<String, Value> = self
            .by_name
            .iter()
            .map(|(k, (n, d))| (k.clone(), Value::Array(vec![Value::from(n.as_str()), Value::from(*d)])))
            .collect();
        let default = match &self.default {
            Some((n, d)) => Value::Array(vec![Value::from(n.as_str()), Value::from(*d)]),
            None => Value::Null,
        };
        let doc = serde_json::json!({ "parts": parts, "default": default });
        let mut buf = Vec::new();
        let fmt = serde_json::ser::PrettyFormatter::with_indent(b" ");
        let mut ser = serde_json::Serializer::with_formatter(&mut buf, fmt);
        serde::Serialize::serialize(&doc, &mut ser).map_err(io::Error::other)?;
        write_atomic(&self.path, &buf)
    }

    /// Assignment of one part (`get`). With `use_default` the default material is returned when the part has none.
    pub fn get(&self, proto_name: &str, use_default: bool) -> Option<Assigned> {
        if let Some((n, d)) = self.by_name.get(proto_name) {
            return Some(Assigned {
                name: n.clone(),
                density: *d,
                explicit: true,
            });
        }
        if use_default {
            if let Some((n, d)) = &self.default {
                return Some(Assigned {
                    name: n.clone(),
                    density: *d,
                    explicit: false,
                });
            }
        }
        None
    }

    /// Assigns a material to several parts and saves (`set`). `None` removes the assignments.
    pub fn set(&mut self, proto_names: &[&str], material: Option<(&str, f64)>) -> io::Result<()> {
        for p in proto_names {
            match material {
                None => {
                    self.by_name.remove(*p);
                }
                Some((n, d)) => {
                    self.by_name.insert(p.to_string(), (n.to_string(), d));
                }
            }
        }
        self.save()
    }

    /// Sets the default material (`None` = do not compute mass for parts without material) and saves.
    pub fn set_default(&mut self, material: Option<(&str, f64)>) -> io::Result<()> {
        self.default = material.map(|(n, d)| (n.to_string(), d));
        self.save()
    }
}

static SAVE_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Writes `bytes` to a temp file of its own next to `path` and renames it over `path`, so a crash or a second window
/// never leaves a half-written assignment file (which would be read as "no materials" and then overwritten).
fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let n = SAVE_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(format!(".{}.{n}.tmp", std::process::id()));
    let tmp = path.with_file_name(name);
    let result = fs::write(&tmp, bytes).and_then(|_| {
        // Windows refuses to replace a file for a moment while another window or a virus scanner has it open
        let mut attempt = 0;
        loop {
            match fs::rename(&tmp, path) {
                Err(_) if attempt < 4 => {
                    attempt += 1;
                    std::thread::sleep(std::time::Duration::from_millis(15 * attempt));
                }
                other => return other,
            }
        }
    });
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn parse_entry(v: &Value) -> Option<(String, f64)> {
    let arr = v.as_array()?;
    if arr.len() != 2 {
        return None;
    }
    let name = arr[0].as_str()?;
    let density = arr[1].as_f64()?;
    if valid_density(density) {
        Some((name.to_string(), density))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_has_exact_densities_in_order() {
        let lib = Library::builtin();
        // 28 entries, from materials.py lines 11-17
        assert_eq!(lib.entries().len(), 28);
        assert_eq!(lib.density("Ocel (konstrukční)"), Some(7.85));
        assert_eq!(lib.density("Nerez (1.4301)"), Some(7.90));
        assert_eq!(lib.density("Litina"), Some(7.20));
        assert_eq!(lib.density("Hliník (slitiny)"), Some(2.70));
        assert_eq!(lib.density("Mosaz"), Some(8.50));
        assert_eq!(lib.density("Měď"), Some(8.96));
        assert_eq!(lib.density("Bronz"), Some(8.80));
        assert_eq!(lib.density("Titan"), Some(4.51));
        assert_eq!(lib.density("Zinek"), Some(7.14));
        assert_eq!(lib.density("PLA"), Some(1.24));
        assert_eq!(lib.density("PETG"), Some(1.27));
        assert_eq!(lib.density("ABS"), Some(1.04));
        assert_eq!(lib.density("ASA"), Some(1.07));
        assert_eq!(lib.density("PA6 / PA12"), Some(1.14));
        assert_eq!(lib.density("POM"), Some(1.41));
        assert_eq!(lib.density("PTFE"), Some(2.20));
        assert_eq!(lib.density("PE-HD"), Some(0.95));
        assert_eq!(lib.density("PP"), Some(0.91));
        assert_eq!(lib.density("PVC"), Some(1.40));
        assert_eq!(lib.density("PC / plexi (PMMA)"), Some(1.19));
        assert_eq!(lib.density("FR4 (DPS)"), Some(1.85));
        assert_eq!(lib.density("Pryž / silikon"), Some(1.20));
        assert_eq!(lib.density("Sklo"), Some(2.50));
        assert_eq!(lib.density("Dřevo (smrk)"), Some(0.45));
        assert_eq!(lib.density("Dřevo (buk)"), Some(0.72));
        assert_eq!(lib.density("MDF / HDF"), Some(0.75));
        assert_eq!(lib.density("Promasil / izolační deska"), Some(0.24));
        assert_eq!(lib.density("Minerální vata"), Some(0.10));
        assert_eq!(lib.density(CUSTOM), None);
        assert_eq!(lib.entries()[0].0, "Ocel (konstrukční)");
        assert_eq!(lib.entries()[27].0, "Minerální vata");
    }

    #[test]
    fn combo_labels_match_python_str_with_decimal_comma() {
        // Python: f"{n}  –  {str(d).replace('.', ',')} g/cm³" with str(7.90) == "7.9", str(2.70) == "2.7"
        assert_eq!(
            Library::combo_label("Nerez (1.4301)", 7.90),
            "Nerez (1.4301)  –  7,9 g/cm³"
        );
        assert_eq!(
            Library::combo_label("Hliník (slitiny)", 2.70),
            "Hliník (slitiny)  –  2,7 g/cm³"
        );
        assert_eq!(
            Library::combo_label("Ocel (konstrukční)", 7.85),
            "Ocel (konstrukční)  –  7,85 g/cm³"
        );
        assert_eq!(
            Library::combo_label("Minerální vata", 0.10),
            "Minerální vata  –  0,1 g/cm³"
        );
    }

    #[test]
    fn custom_name_uses_g_format_and_decimal_comma() {
        // Python: f"Vlastní ({value:g} g/cm³)".replace(".", ",")
        assert_eq!(custom_name(7.85), "Vlastní (7,85 g/cm³)");
        assert_eq!(custom_name(8.0), "Vlastní (8 g/cm³)");
        assert_eq!(custom_name(0.001), "Vlastní (0,001 g/cm³)");
        assert_eq!(custom_name(12.345678), "Vlastní (12,3457 g/cm³)");
    }

    #[test]
    fn density_validity_bounds() {
        assert!(valid_density(0.001));
        assert!(valid_density(99.9));
        assert!(!valid_density(0.0));
        assert!(!valid_density(100.0));
        assert!(!valid_density(-1.0));
        assert!(!valid_density(f64::NAN));
    }

    #[test]
    fn store_path_is_under_materials_with_sha1_of_normcase_path() {
        let dir = tempfile::tempdir().unwrap();
        let model = dir.path().join("Model.STEP");
        let p = store_path(dir.path(), &model).unwrap();
        assert_eq!(p.parent().unwrap(), dir.path().join("materials"));
        assert!(p.to_string_lossy().ends_with(".json"));
        // key = sha1 of normcase(abspath) of the model. Python: hashlib.sha1(ntpath.normcase(ntpath.abspath(
        // "C:/Models/A.step")).encode()) gives this digest for the text c:\models\a.step
        assert_eq!(
            paths::sha1_hex("c:\\models\\a.step"),
            "7a2d012dbf00277d8d933dfae7fdd8a7fabac114"
        );
    }

    #[test]
    fn assignments_save_load_and_get_semantics() {
        let cache = tempfile::tempdir().unwrap();
        let model = cache.path().join("m.step");
        let mut a = Assignments::load(cache.path(), &model).unwrap();
        assert!(a.by_name.is_empty());
        assert_eq!(a.get("Part1", true), None);

        a.set(&["Part1", "Part2"], Some(("Litina", 7.20))).unwrap();
        a.set_default(Some(("PLA", 1.24))).unwrap();

        let b = Assignments::load(cache.path(), &model).unwrap();
        assert_eq!(b.by_name.len(), 2);
        assert_eq!(
            b.get("Part1", false),
            Some(Assigned { name: "Litina".into(), density: 7.20, explicit: true })
        );
        // not assigned: default is used only when asked for
        assert_eq!(b.get("Other", false), None);
        assert_eq!(
            b.get("Other", true),
            Some(Assigned { name: "PLA".into(), density: 1.24, explicit: false })
        );
        // removing an assignment
        a.set(&["Part1"], None).unwrap();
        let c = Assignments::load(cache.path(), &model).unwrap();
        assert!(c.get("Part1", false).is_none());
        assert!(c.get("Part2", false).is_some());
    }

    #[test]
    fn assignments_file_layout_matches_python() {
        let cache = tempfile::tempdir().unwrap();
        let model = cache.path().join("m.step");
        let mut a = Assignments::load(cache.path(), &model).unwrap();
        a.set(&["Ø10 hřídel"], Some(("Ocel (konstrukční)", 7.85))).unwrap();
        let text = fs::read_to_string(a.path()).unwrap();
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["parts"]["Ø10 hřídel"], serde_json::json!(["Ocel (konstrukční)", 7.85]));
        assert_eq!(v["default"], Value::Null);
        assert!(text.contains("\n \"parts\""), "indent of one space:\n{text}");
        assert!(text.contains("Ø10 hřídel"), "UTF-8 is written unescaped");
    }

    #[test]
    fn saving_is_atomic_and_leaves_no_temp_file() {
        let cache = tempfile::tempdir().unwrap();
        let model = cache.path().join("m.step");
        let mut a = Assignments::load(cache.path(), &model).unwrap();
        a.set(&["A"], Some(("Litina", 7.20))).unwrap();
        let first = fs::read(a.path()).unwrap();
        a.set(&["B"], Some(("PLA", 1.24))).unwrap();
        // the file was replaced as a whole, and only the assignment file is in the folder
        let names: Vec<String> = fs::read_dir(a.path().parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names.len(), 1, "{names:?}");
        assert!(names[0].ends_with(".json"));
        assert_ne!(fs::read(a.path()).unwrap(), first);
        // a failed write (the target is a folder) removes its temp file and reports the error
        let blocker = cache.path().join("materials").join("blocked.json");
        fs::create_dir_all(&blocker).unwrap();
        assert!(write_atomic(&blocker, b"{}").is_err());
        let leftovers = fs::read_dir(cache.path().join("materials")).unwrap().filter(|e| {
            e.as_ref().unwrap().file_name().to_string_lossy().ends_with(".tmp")
        });
        assert_eq!(leftovers.count(), 0);
    }

    #[test]
    fn two_saves_at_once_never_share_a_temp_file() {
        let cache = tempfile::tempdir().unwrap();
        let model = cache.path().join("m.step");
        let dir = cache.path().to_path_buf();
        let handles: Vec<_> = (0..4)
            .map(|i| {
                let (dir, model) = (dir.clone(), model.clone());
                std::thread::spawn(move || {
                    let mut a = Assignments::load(&dir, &model).unwrap();
                    for k in 0..10 {
                        // one of the writers can lose a race for the target; what matters is the state afterwards
                        let _ = a.set(&[format!("T{i}-{k}").as_str()], Some(("PLA", 1.24)));
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        // whatever the last writer left is a complete, readable file, and no writer left a temp file behind
        let b = Assignments::load(&dir, &model).unwrap();
        assert!(!b.by_name.is_empty());
        let text = fs::read_to_string(b.path()).unwrap();
        serde_json::from_str::<Value>(&text).expect("a complete JSON file");
        let tmp = fs::read_dir(b.path().parent().unwrap()).unwrap().filter(|e| {
            e.as_ref().unwrap().file_name().to_string_lossy().ends_with(".tmp")
        });
        assert_eq!(tmp.count(), 0);
    }

    #[test]
    fn damaged_entries_are_dropped() {
        let cache = tempfile::tempdir().unwrap();
        let model = cache.path().join("m.step");
        let a = Assignments::load(cache.path(), &model).unwrap();
        fs::create_dir_all(a.path().parent().unwrap()).unwrap();
        fs::write(
            a.path(),
            r#"{"parts": {"ok": ["Litina", 7.2], "bad": ["x", 500], "short": ["x"], "str": ["x", "7.2"]},
                "default": ["Y", 0]}"#,
        )
        .unwrap();
        let b = Assignments::load(cache.path(), &model).unwrap();
        assert_eq!(b.by_name.keys().collect::<Vec<_>>(), vec!["ok"]);
        assert_eq!(b.default, None);
    }
}
